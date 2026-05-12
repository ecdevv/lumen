//! TUI state model.
//!
//! Owns the conversation timeline, the editable input buffer, the
//! shared `Arc<Mutex<Agent>>` handle and its current turn task,
//! input-history (in-memory ring + on-disk JSONL persistence at
//! `<data_dir>/input_history`), and the various flags the renderer
//! reads (mode, esc-armed, scroll offset, spinner tick, show_help).

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lumen_core::{Agent, AgentEvent, AutoApply, Config, Verdict};
use ratatui::style::Style;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tui_textarea::TextArea;

use super::timeline::Timeline;

/// Maximum number of recent submissions kept in the history ring buffer.
/// Mirrors to disk at `<data_dir>/input_history` when
/// [`HistoryState::path`] is set. Bumped from 50 (in-memory-only) to
/// 1000 now that the buffer persists across sessions.
pub const HISTORY_CAPACITY: usize = 1000;

/// Filename of the persistent input-history file under
/// [`lumen_core::PathsConfig::data_dir`].
const HISTORY_FILE_NAME: &str = "input_history";

/// Resolve the on-disk input-history path for the given data directory.
#[must_use]
pub fn input_history_path(data_dir: &Path) -> PathBuf {
    data_dir.join(HISTORY_FILE_NAME)
}

/// Prettify a path for status-bar display: collapse the user's home
/// directory to `~`. Falls back to the raw path display when HOME is
/// unset (containers, etc.) or when the path is outside HOME.
//
// HOME is read once and cached: this runs in the status-bar render path,
// fired on every keystroke and 10x/sec while streaming, so re-querying
// the env each time is needless syscall churn.
#[must_use]
pub fn pretty_path(p: &Path) -> String {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    let home = HOME.get_or_init(|| {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    });
    if let Some(home) = home
        && let Ok(rel) = p.strip_prefix(home)
    {
        if rel.as_os_str().is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", rel.display());
    }
    p.display().to_string()
}

/// Render a path for the UI, preferring cwd-relative form so that
/// file operations inside the active project don't drown the
/// conversation pane in long absolute paths. Order: cwd-relative
/// (`test.txt`), then home-relative via [`pretty_path`]
/// (`~/Documents/...`), then absolute. Outside-cwd paths keeping
/// their absolute form is intentional - it's a leak signal that
/// the operation isn't on a project file.
///
/// Single source of truth for "how does a path render in the UI";
/// every render site (tool call line, approval header, diff body,
/// streaming spinner) routes through here.
#[must_use]
pub fn display_path(p: &Path, cwd: &Path) -> String {
    if let Ok(rel) = p.strip_prefix(cwd) {
        if rel.as_os_str().is_empty() {
            return ".".to_string();
        }
        return rel.display().to_string();
    }
    pretty_path(p)
}

/// Load the persistent input history. Returns an empty deque if the
/// file is missing, unreadable, or empty.
//
// Format: JSON-Lines. One JSON-encoded string per line. JSON is used
// instead of plain text so multi-line submissions (Shift/Alt+Enter)
// round-trip without escape gymnastics.
//
// On load: trim to capacity AND deduplicate (mode B / erasedups),
// keeping each entry's last occurrence. If anything changed, rewrite
// the file so the on-disk view stays in sync with the policy.
//
// Also self-heals an orphan `.tmp` sibling: a crash between
// `tokio::fs::write(&tmp, ...)` and `rename(&tmp, path)` in
// `save_full_history` would otherwise accumulate `input_history.tmp`
// files in the data dir forever. Removing it on each load is
// cheap and keeps the directory tidy.
pub async fn load_input_history(path: &Path) -> VecDeque<String> {
    let _ = tokio::fs::remove_file(path.with_extension("tmp")).await;
    let Ok(content) = tokio::fs::read_to_string(path).await else {
        return VecDeque::new();
    };
    let raw: Vec<String> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<String>(line).ok())
        .collect();
    let original_len = raw.len();

    // Walk in reverse, keep first sighting of each unique entry; that
    // gives us "last occurrence wins" once we re-reverse. Faster than a
    // forward retain that has to scan the rest of the buffer per entry.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<String> = raw
        .into_iter()
        .rev()
        .filter(|e| seen.insert(e.clone()))
        .collect();
    deduped.reverse();

    // Cap to capacity (oldest go).
    if deduped.len() > HISTORY_CAPACITY {
        deduped.drain(0..deduped.len() - HISTORY_CAPACITY);
    }

    let entries: VecDeque<String> = deduped.into();
    if entries.len() != original_len {
        let _ = save_full_history(path, &entries).await;
    }
    entries
}

/// Rewrite the entire history file from `entries`. Used by the
/// startup trim/dedup path and the single saver task spawned by
/// [`spawn_history_saver`] - never called from `HistoryState::push`
/// directly, so cross-task locking isn't needed: the architecture
/// guarantees at most one concurrent caller.
//
// Atomicity: plain `fs::write(path, ...)` is `open(O_TRUNC) + write
// + close`, none of which is atomic. A crash mid-write leaves a
// truncated file; the next load can't parse it and silently drops
// history. We write to `path.tmp` then `rename(tmp, path)` -
// `rename(2)` is atomic for same-filesystem renames on POSIX, and
// `MoveFileEx(MOVEFILE_REPLACE_EXISTING)` on Windows is too. The
// orphan-tmp cleanup in `load_input_history` handles the
// crash-between-write-and-rename window.
async fn save_full_history(path: &Path, entries: &VecDeque<String>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut content = String::new();
    for entry in entries {
        if let Ok(line) = serde_json::to_string(entry) {
            content.push_str(&line);
            content.push('\n');
        }
    }

    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, path).await
}

/// Spawn the long-lived task that owns every write to the on-disk
/// input-history file. Returns a sender that [`HistoryState::push`]
/// feeds snapshots through, plus the task's [`JoinHandle`] (tests
/// await it after dropping the sender; production fire-and-forgets).
///
/// On each iteration the task awaits one snapshot then drains any
/// further queued snapshots synchronously via `try_recv`, writing
/// only the latest. A burst of N submits collapses to O(1)..O(N)
/// writes (usually 1) and the most-recent snapshot always wins.
//
// Replaces the prior `tokio::spawn`-per-push pattern, which could
// lose entries when two spawned saves raced the same tmp file under
// multi-thread scheduling - the second spawn might run first, then
// the first spawn's stale snapshot would `rename` over the newer
// state. Single-writer-by-construction sidesteps the ordering
// problem entirely; no cross-task lock needed in
// `save_full_history`.
//
// The task exits when every clone of the sender is dropped (i.e.
// when `AppState` drops on TUI exit).
pub fn spawn_history_saver(
    path: PathBuf,
) -> (UnboundedSender<VecDeque<String>>, JoinHandle<()>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<VecDeque<String>>();
    let handle = tokio::spawn(async move {
        while let Some(mut snapshot) = rx.recv().await {
            // Coalesce: keep pulling synchronously-available newer
            // snapshots so a burst of N pushes results in one
            // write of the latest, not N writes of intermediate
            // states.
            while let Ok(newer) = rx.try_recv() {
                snapshot = newer;
            }
            if let Err(e) = save_full_history(&path, &snapshot).await {
                tracing::warn!(error = %e, "input_history save failed");
            }
        }
    });
    (tx, handle)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Idle,
    Streaming,
}

/// Inactivity window during which a second tap on the same key
/// (Esc / Ctrl+C) confirms the chord. Past this point, the armed
/// state silently disarms and the status hint disappears. 2s is the
/// chord window most users expect from bash/zsh-style double-tap quits.
pub const ARM_TIMEOUT: Duration = Duration::from_secs(2);

/// Which key armed the current double-tap chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmedKey {
    Esc,
    CtrlC,
}

/// A pending double-tap chord: which key armed it and when.
/// [`AppState::armed_key`] returns `None` once `at.elapsed() >= ARM_TIMEOUT`,
/// so the field's `Some(_)` state is the *raw* arm regardless of
/// expiry - callers must consult `armed_key()` for the live view.
#[derive(Debug, Clone, Copy)]
pub struct ArmState {
    pub key: ArmedKey,
    pub at: Instant,
}

impl ArmState {
    pub fn new(key: ArmedKey) -> Self {
        Self { key, at: Instant::now() }
    }
}

/// One mouse selection in the conversation pane. Coordinates are
/// absolute frame cells (col, row); the renderer compares against the
/// buffer's current `area` to decide which cells to highlight.
//
// Anchored to screen position rather than content-line index. As a
// trade-off, scrolling the conversation while a selection is visible
// would visually drift; we clear the selection on scroll to keep things
// honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Cell where the drag started. (col, row).
    pub anchor: (u16, u16),
    /// Cell where the drag is currently / was last. (col, row).
    pub focus: (u16, u16),
}

impl Selection {
    /// Returns `(top_left, bottom_right)` in row-major order, regardless
    /// of which way the user dragged.
    #[must_use]
    pub fn normalized(self) -> ((u16, u16), (u16, u16)) {
        // Compare by (row, col) so selections that span lines order
        // top-to-bottom; left-to-right within a single line falls out.
        if (self.anchor.1, self.anchor.0) <= (self.focus.1, self.focus.0) {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }
}

/// One choice inside the approval menu. Maps to a verdict (Accept /
/// Reject) and, for `AcceptAll`, an additional side effect of
/// flipping the session's `auto_apply` policy.
//
// Variants and their availability are kind-specific - see
// [`approval_options`]. Adding a new option requires deciding which
// kinds offer it and where in [`apply_approval_option`] its side
// effects fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOption {
    /// Apply this one operation; don't change policy.
    Accept,
    /// Apply this operation AND auto-accept future ones of the same
    /// kind by flipping `auto_apply` (diff -> Safe; shell currently
    /// doesn't offer this until per-command allowlisting lands).
    AcceptAll,
    /// Refuse this operation. No policy change.
    Reject,
}

/// Available approval options for the given kind, in display order.
/// Each tuple is `(option, label, letter-shortcut)`. The shortcut is
/// the single key (lowercase) that immediately applies the option
/// without requiring Up/Down + Enter navigation.
//
// Diff: 3 options including "Accept all this session" (flips to
// Safe so subsequent edits auto-apply).
//
// Shell: 2 options. There is no "Accept all" tier because there's
// no `Always`-equivalent policy for shell; per-command allowlisting
// is the future shape for letting trusted shells through, and it
// hasn't landed yet. The `a` shortcut therefore does nothing on a
// shell prompt rather than silently mapping to a different option.
#[must_use]
pub fn approval_options(kind: &ApprovalKind) -> &'static [(ApprovalOption, &'static str, char)] {
    match kind {
        ApprovalKind::Diff { .. } => DIFF_OPTIONS,
        ApprovalKind::Shell { .. } => SHELL_OPTIONS,
    }
}

const DIFF_OPTIONS: &[(ApprovalOption, &str, char)] = &[
    (ApprovalOption::Accept, "Accept", 'y'),
    (ApprovalOption::AcceptAll, "Accept all this session", 'a'),
    (ApprovalOption::Reject, "Reject", 'n'),
];

const SHELL_OPTIONS: &[(ApprovalOption, &str, char)] = &[
    (ApprovalOption::Accept, "Accept", 'y'),
    (ApprovalOption::Reject, "Reject", 'n'),
];

/// What kind of approval the UI must surface in the modal overlay.
//
// Lives on the CLI side (not core) because it's a presentation
// concern - core's [`lumen_core::ApprovalGate`] trait already
// separates `review_diff` from `review_shell` at the API surface.
// This enum unifies them for the modal-rendering pipeline.
#[derive(Debug)]
pub enum ApprovalKind {
    /// A proposed file edit. Modal renders the unified diff and
    /// the target path.
    Diff {
        /// Target file (already sandboxed).
        path: PathBuf,
        /// Unified diff string built by [`lumen_core::diff::unified_diff`].
        diff: String,
    },
    /// A proposed shell command. Modal renders the verbatim command.
    Shell {
        /// Command string the model produced.
        command: String,
    },
}

/// One approval request awaiting the user's verdict. While
/// [`AppState::pending_approval`] is `Some`, the inline preview
/// renders and `input::handle_key` intercepts ↑/↓ / Enter / y / a / n / Esc.
#[derive(Debug)]
pub struct PendingApproval {
    pub kind: ApprovalKind,
    /// One-shot reply channel back to the waiting tool. Sending
    /// either verdict unblocks `Tool::invoke`; dropping the
    /// sender (e.g., via `cancel_turn`) signals the tool to treat
    /// the request as a Reject via the receiver's RecvError path.
    pub reply: oneshot::Sender<Verdict>,
    /// Currently-highlighted menu option (0 = Accept, 1 = Accept all
    /// this session, 2 = Reject). Driven by Up/Down arrows; Enter
    /// confirms whichever index is here. Defaults to 0 (Accept) on
    /// fresh requests, matching Claude Code's prompt default.
    pub selected: usize,
}

/// Messages flowing from spawned agent tasks back to the UI loop.
//
// Wraps `AgentEvent` plus free-form notes - for transport errors, agent
// task failures, and user-initiated cancellation acks. Keeps lumen-core's
// `AgentEvent` focused on model output. `ApprovalRequest` carries the
// reply channel that the tool's `Tool::invoke` is awaiting.
//
// `Debug` derive: `oneshot::Sender<Verdict>` itself derives Debug, so
// the macro Just Works here. Useful for `tracing::trace!(?msg, ...)`
// and for test panic-format diagnostics.
#[derive(Debug)]
pub enum UiMsg {
    Agent(AgentEvent),
    Note(String),
    ApprovalRequest {
        kind: ApprovalKind,
        reply: oneshot::Sender<Verdict>,
    },
    /// Async result of `Provider::list_models()` for the model
    /// picker. `Ok(list)` populates the picker's loaded state;
    /// `Err(msg)` puts it into the error state for display.
    /// Ignored if the user has already closed the picker.
    ModelsLoaded(Result<Vec<String>, String>),
}

/// Persistent input-history ring with mode-B (erasedups) policy.
/// Bounded by [`HISTORY_CAPACITY`]; on push, prior copies of the entry
/// are removed before append so each Up-arrow recall returns a different
/// prompt. Mirrors to disk at [`Self::path`] when set.
#[derive(Debug, Default)]
pub struct HistoryState {
    /// Ring buffer of recent submissions, newest at the back.
    pub entries: VecDeque<String>,
    /// Index in `entries` while recalling. `None` = currently editing
    /// fresh input (not browsing history).
    pub cursor: Option<usize>,
    /// Snapshot of the input buffer when the user starts walking
    /// history; restored when they walk past the newest entry.
    pub draft: Option<String>,
    /// Channel to the long-lived saver task. `None` for tests and
    /// ephemeral sessions; production wires it via `super::run`
    /// after [`spawn_history_saver`]. Every [`Self::push`] sends a
    /// snapshot through this channel - the task coalesces bursts
    /// and serializes writes so no two saves can race the same
    /// tmp file.
    pub saver: Option<UnboundedSender<VecDeque<String>>>,
}

impl HistoryState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(HISTORY_CAPACITY),
            cursor: None,
            draft: None,
            saver: None,
        }
    }

    /// Push a submission onto the ring (mode B / erasedups): drop any
    /// prior copies of `text`, then append. Drops the oldest entry if
    /// at capacity. Hands the latest snapshot to the saver task when
    /// [`Self::saver`] is set.
    //
    // `send` is sync and channel-FIFO; the receiving task coalesces
    // bursts so the on-disk file always converges to the most recent
    // snapshot. A `send` error means the saver task has already
    // exited (shutdown path) - dropping silently is correct.
    pub fn push(&mut self, text: String) {
        self.entries.retain(|e| e != &text);
        if self.entries.len() == HISTORY_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(text);
        self.cursor = None;
        self.draft = None;

        if let Some(saver) = &self.saver {
            let _ = saver.send(self.entries.clone());
        }
    }
}

/// Transient render-loop bookkeeping. Lives separately from semantic
/// app state because these fields exist purely for one-frame
/// coordination between the event loop, the renderer, and the
/// post-draw OSC 52 push:
///
/// * [`Self::spinner_tick`] - bumped by the redraw timer, mod-indexed
///   into `SPINNER_FRAMES` by the renderer.
/// * [`Self::last_rendered_scroll`] - written by the renderer at the
///   end of each frame; the next frame compares against the current
///   scroll to compute the *applied* scroll delta and shift any active
///   selection by that amount.
/// * [`Self::copy_pending`] - set on mouse-up after a non-empty drag;
///   the renderer reads this the next frame, extracts text from the
///   post-paint buffer, and fills [`Self::clipboard_pending`].
/// * [`Self::clipboard_pending`] - selected text waiting for the
///   event loop to write via OSC 52 after `terminal.draw` returns.
#[derive(Debug, Default)]
pub struct RenderState {
    pub spinner_tick: usize,
    pub last_rendered_scroll: usize,
    pub copy_pending: bool,
    pub clipboard_pending: Option<String>,
    /// Inner textarea width (in cells) from the last rendered frame.
    /// Stashed here so `input::handle_up` / `handle_down` can do
    /// visual-row-aware cursor movement: a long pasted line wraps
    /// into multiple visual rows, and Up should move one visual
    /// row up (not jump to the start of the logical line). `0`
    /// until the first render lands, which is fine - the first
    /// frame draws before any keystrokes are accepted by the
    /// event loop. Width-0 callers fall back to logical-row
    /// movement.
    pub last_textarea_width: u16,
}

/// Top-level application state.
pub struct AppState {
    pub cfg: Config,
    pub timeline: Timeline,
    pub agent_tx: UnboundedSender<UiMsg>,
    pub input: TextArea<'static>,
    /// Shared with each spawned `agent.turn` task.
    pub agent: Arc<Mutex<Agent>>,
    pub mode: AppMode,
    /// Handle on the in-flight turn task, if any. Used to abort on
    /// cancel; cleared on `TurnEnd` / `Note` events and on cancel.
    pub turn_handle: Option<JoinHandle<()>>,
    /// Tracks a pending double-tap chord (Esc or Ctrl+C). The second
    /// press of the same key within [`ARM_TIMEOUT`] confirms; any other
    /// key resets the field; passing the timeout silently disarms (the
    /// status hint also disappears). `None` = no chord in progress.
    pub arm_state: Option<ArmState>,
    /// Persistent input history (entries + cursor + draft + on-disk
    /// path). See [`HistoryState`].
    pub history: HistoryState,
    /// Working directory the agent operates from. Set at startup;
    /// rendered (prettified) in the status bar so the user always sees
    /// where tools will dispatch.
    pub cwd: PathBuf,
    /// First 8 chars of the running session's UUID. Stable across the
    /// session; rendered in the status bar so users can correlate the
    /// running session with its on-disk transcript file.
    pub session_id_short: String,
    /// `true` while the help overlay is shown (toggled with `?`).
    pub show_help: bool,
    /// `Some` while the slash-command palette is open. Holds the
    /// selected-row index; the query itself lives in the input
    /// buffer (we use the textarea as the search box so the user
    /// sees their typing in the familiar place). `None` = palette
    /// closed and `/` behaves as a literal char.
    pub slash_palette: Option<super::slash::SlashPalette>,
    /// `Some` while the model picker is open (opened by `/model`
    /// with no args). The picker reuses the input buffer as a
    /// filter just like the slash palette. State carries the
    /// async-fetch lifecycle (loading / loaded / error).
    pub model_picker: Option<super::model_picker::ModelPickerState>,
    /// Cached model list from the last successful
    /// `Provider::list_models()` call. Reused on subsequent
    /// `/model` opens so the picker shows results instantly
    /// instead of cycling through a Loading state on every open.
    /// Invalidation policy: session-scoped (no TTL). Restart to
    /// refresh if the server's model set changes.
    pub cached_models: Option<Vec<String>>,
    /// Path of the TOML config file backing `cfg`, if any. `None`
    /// when the user is running with compiled defaults (no file
    /// resolvable). The `/model <name>` slash command writes
    /// changes back to this path so they survive restarts;
    /// `None` falls back to in-memory only with a timeline note.
    pub cfg_path: Option<PathBuf>,
    /// `Some` while the `/settings` overlay is open. Holds the
    /// selected-field index and (optionally) an active edit
    /// buffer. The modal renders centered like the help overlay
    /// but with type-aware edit interactions.
    pub settings: Option<super::settings::SettingsState>,
    /// Lines scrolled up from the bottom of the conversation viewport.
    /// 0 = anchored to bottom (auto-scroll on new content). Mouse-wheel
    /// up increments this; wheel-down decrements toward 0.
    pub scroll_offset: usize,
    /// Active mouse-driven selection, if any. The renderer paints these
    /// cells with `Modifier::REVERSED`. Cleared on a click without drag,
    /// when a fresh drag starts, etc.
    pub selection: Option<Selection>,
    /// Approval request from an in-flight tool, awaiting user verdict.
    /// When `Some`: the modal overlay renders, input handling
    /// intercepts y/n/Esc, all other keystrokes are swallowed.
    /// Cleared on verdict-sent, on `cancel_turn`, and on quit.
    pub pending_approval: Option<PendingApproval>,
    /// Shared approval-policy handle. Same `Arc` is held by the
    /// `ToolContext` (so tools read the current mode) and the
    /// `TuiApprovalGate` (which auto-accepts file edits under
    /// `Safe`; shell always prompts regardless of mode).
    /// Shift+Tab in `input::handle_key` mutates this; readers
    /// observe the change immediately via `Relaxed` atomic load.
    pub auto_apply: Arc<AtomicU8>,
    /// Latest tool name + arguments while a tool is conceptually
    /// "active." Set on `ToolCallStart`; kept across `ToolCallEnd`
    /// (which is the key bit - fast tools end before the next
    /// render frame, so the `Running` status would otherwise never
    /// be visible); cleared on the first `AssistantText` after End
    /// (model is responding to the tool result now) or on
    /// `TurnEnd` / `Note` / `cancel_turn`. Drives the streaming
    /// spinner's dynamic label ("Reading X…" / "Running: cmd").
    pub active_tool: Option<(String, String)>,
    /// Transient render-loop coordination state. See [`RenderState`].
    pub render: RenderState,
}

impl AppState {
    pub fn new(
        cfg: Config,
        agent_tx: UnboundedSender<UiMsg>,
        agent: Arc<Mutex<Agent>>,
    ) -> Self {
        Self {
            cfg,
            timeline: Timeline::new(),
            agent_tx,
            input: make_input_textarea(),
            agent,
            mode: AppMode::Idle,
            turn_handle: None,
            arm_state: None,
            history: HistoryState::new(),
            cwd: PathBuf::from("."),
            session_id_short: String::new(),
            show_help: false,
            slash_palette: None,
            model_picker: None,
            cached_models: None,
            cfg_path: None,
            settings: None,
            scroll_offset: 0,
            selection: None,
            pending_approval: None,
            // Default-initialized; production wiring overwrites this
            // with the shared `Arc` from `build_agent` so the
            // ToolContext and the gate see the same cell. Tests use
            // the default and mutate via `set_auto_apply` if needed.
            auto_apply: Arc::new(AtomicU8::new(AutoApply::default().as_u8())),
            active_tool: None,
            render: RenderState::default(),
        }
    }

    /// Snapshot the current approval policy. Convenience wrapper
    /// around the atomic load.
    pub fn auto_apply(&self) -> AutoApply {
        AutoApply::from_u8(self.auto_apply.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Update the approval policy. Visible immediately to every
    /// `ToolContext::auto_apply()` reader and to the gate.
    pub fn set_auto_apply(&self, mode: AutoApply) {
        self.auto_apply
            .store(mode.as_u8(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Reset the input buffer to empty. Replaces the textarea wholesale,
    /// so the undo stack and yank buffer are wiped - by design: the
    /// Esc/Ctrl+C clear is *final*, retype to recover. (Per-character
    /// undo via Ctrl+Z within an in-progress edit still works because
    /// it's tui-textarea's default; users who want their last
    /// submission back use the Up arrow.)
    pub fn reset_input(&mut self) {
        self.input = make_input_textarea();
    }

    /// `true` when the input buffer has no non-whitespace content.
    pub fn input_is_empty(&self) -> bool {
        self.input.lines().iter().all(|l| l.trim().is_empty())
    }

    /// The currently armed key, if any, *and* still within the
    /// [`ARM_TIMEOUT`] window. Returns `None` once the chord has
    /// expired so callers and the status bar treat it as disarmed
    /// without needing a tick to clear the field.
    pub fn armed_key(&self) -> Option<ArmedKey> {
        self.arm_state
            .as_ref()
            .filter(|s| s.at.elapsed() < ARM_TIMEOUT)
            .map(|s| s.key)
    }

    /// Replace the input buffer's content with `text`. Used when
    /// recalling from history.
    pub fn set_input(&mut self, text: &str) {
        let mut t = make_input_textarea();
        // `insert_str` accepts multi-line content; tui-textarea splits
        // it into lines internally.
        t.insert_str(text);
        self.input = t;
    }

    /// Apply one [`UiMsg`]. Drives [`Timeline::apply`] for agent events
    /// and pushes notes verbatim; both terminal cases (turn-end, note)
    /// flip back to [`AppMode::Idle`] and drop the spawned-task handle.
    /// [`UiMsg::ApprovalRequest`] parks the modal-pending state - the
    /// turn task remains alive, awaiting the reply.
    pub fn apply_ui_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Agent(event) => {
                // Drive the streaming-spinner's active-tool tracking
                // BEFORE handing the event to Timeline::apply (which
                // consumes by value).
                //
                // Semantics: active_tool reflects "what's happening
                // right now," not "what just happened." It's set on
                // Start and cleared on End so the spinner accurately
                // returns to "thinking…" the moment the tool
                // finishes - even though the model usually takes
                // seconds to respond to the tool result, the spinner
                // shouldn't keep saying "Reading X…" during that
                // post-tool wait.
                //
                // The "fast tools never get a Running frame" problem
                // is solved at the event-loop layer instead: the
                // drain in `tui/mod.rs` force-renders after applying
                // a ToolCallStart so the dynamic label is guaranteed
                // to land in at least one frame.
                match &event {
                    AgentEvent::ToolCallStart { name, arguments, .. } => {
                        self.active_tool = Some((name.clone(), arguments.clone()));
                    }
                    AgentEvent::ToolCallEnd { .. } => {
                        self.active_tool = None;
                    }
                    _ => {}
                }
                // `Timeline::apply` returns true on `TurnEnd`.
                if self.timeline.apply(event) {
                    self.mode = AppMode::Idle;
                    self.turn_handle = None;
                    // Defensive: TurnEnd shouldn't fire while a
                    // tool is awaiting approval (the turn can't end
                    // mid-dispatch), but if the agent task died
                    // for any reason we don't want a stale modal.
                    self.pending_approval = None;
                    self.active_tool = None;
                }
            }
            UiMsg::Note(text) => {
                self.timeline.push_note(text);
                self.mode = AppMode::Idle;
                self.turn_handle = None;
                self.pending_approval = None;
                self.active_tool = None;
            }
            UiMsg::ApprovalRequest { kind, reply } => {
                // Overwriting an existing pending approval would
                // strand the prior tool's reply channel - tools
                // dispatch sequentially so this shouldn't happen,
                // but log if it does so the bug surfaces.
                if self.pending_approval.is_some() {
                    tracing::warn!(
                        "stacking approval request while one is already pending; \
                         the prior request will be dropped (tool will see Reject)"
                    );
                }
                self.pending_approval = Some(PendingApproval {
                    kind,
                    reply,
                    selected: 0,
                });
            }
            UiMsg::ModelsLoaded(result) => {
                // Cache the successful result regardless of whether
                // the picker is still open - the user might
                // reopen and we'd rather have warm data than
                // refetch. Errors don't populate the cache (next
                // open should retry).
                if let Ok(models) = &result {
                    self.cached_models = Some(models.clone());
                }
                // Drop silently if the picker was closed before
                // the async fetch returned - the user moved on.
                if let Some(picker) = self.model_picker.as_mut() {
                    picker.selected = 0;
                    picker.status = match result {
                        Ok(models) => super::model_picker::ModelPickerStatus::Loaded {
                            models,
                        },
                        Err(message) => {
                            super::model_picker::ModelPickerStatus::Error { message }
                        }
                    };
                }
            }
        }
    }
}

fn make_input_textarea() -> TextArea<'static> {
    let mut t = TextArea::default();
    // The surrounding `Block` is set by `render::render_input` each
    // frame, so we don't attach one here. tui-textarea defaults
    // `cursor_line_style` to UNDERLINED, which shows up as a
    // distracting line under the row the cursor is on; the plain
    // default keeps the row visually quiet.
    t.set_cursor_line_style(Style::default());
    // Placeholder text is rendered by our custom wrap-aware render
    // path (`render::render_input` -> `INPUT_PLACEHOLDER`), not by
    // tui-textarea, so we don't call `set_placeholder_text` here.
    // tui-textarea's render is bypassed entirely - we only use it
    // as the edit-state container (cursor, undo, history routing).
    t
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Continue,
    Quit,
}

#[cfg(test)]
#[path = "../tests/tui/app.rs"]
mod tests;
