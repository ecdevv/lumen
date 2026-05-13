//! Terminal UI entry point.
//!
//! Owns the terminal lifecycle (raw mode + alt-screen with a panic-safe
//! RAII guard, mouse capture, optional Kitty keyboard protocol) and the
//! async event loop that selects on keystrokes, mouse events, agent
//! channel messages, and the spinner redraw timer. The agent runs in a
//! spawned task; this module is the orchestrator.

mod app;
mod approval;
mod clipboard;
mod input;
mod markdown;
mod model_picker;
mod render;
mod settings;
mod slash;
mod timeline;

#[cfg(test)]
#[path = "../tests/tui/test_support.rs"]
mod test_support;

use std::io::{Stdout, Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use lumen_core::provider::HttpProvider;
use lumen_core::{
    Agent, AgentEvent, AgentOptions, CORE_SYSTEM_PROMPT, Config, Session, ToolContext,
    ToolRegistry,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::Mutex;
use tokio::sync::mpsc::unbounded_channel;

use self::app::{
    Action, AppState, UiMsg, input_history_path, load_input_history, spawn_history_saver,
};
use self::approval::TuiApprovalGate;

/// Launch the TUI. Constructs the provider/session/agent, runs the
/// event loop, restores the terminal on exit.
pub async fn run(cfg: Config, cfg_path: Option<PathBuf>, cwd: PathBuf) -> Result<()> {
    seed_config_template_if_missing(cfg_path.as_deref(), &cfg);

    // Channel must exist before agent construction so the
    // TuiApprovalGate (held by ToolContext) can route approval
    // requests back into the UI loop.
    //
    // Unbounded: AgentEvent emission is sync (`FnMut` callback) so
    // bounded `send` (which is async) can't be wired here without
    // changing the callback shape. Producer is naturally rate-limited
    // by network / model speed, so unbounded growth isn't a real risk.
    let (tx, mut rx) = unbounded_channel::<UiMsg>();

    let (agent, auto_apply) = build_agent(&cfg, cwd.clone(), tx.clone()).await?;

    // Read the session id once, sync, before the agent starts handling
    // turns. The mutex is uncontended at this point.
    let session_id_short = {
        let agent_locked = agent.lock().await;
        let full = agent_locked.session().id().to_string();
        // First octet of the UUID (8 hex chars) is enough for visual
        // disambiguation between concurrent sessions.
        full.chars().take(8).collect::<String>()
    };

    // Load persistent input history before constructing AppState so
    // Up arrow on the first turn can recall previous-session submissions.
    let history_path = input_history_path(&cfg.paths.data_dir);
    let loaded_history = load_input_history(&history_path).await;
    // Spawn the single saver task that owns every write to the
    // on-disk history file. JoinHandle is fire-and-forget: when
    // AppState drops on exit, its sender drops, the task drains
    // its final snapshot, then exits.
    let (history_tx, _history_saver) = spawn_history_saver(history_path);

    let mut terminal = setup_terminal()?;
    let _guard = TerminalGuard;

    let mut app = AppState::new(cfg, tx, agent);
    app.cfg_path = cfg_path;
    app.history.entries = loaded_history;
    app.history.saver = Some(history_tx);
    app.cwd = cwd;
    app.session_id_short = session_id_short;
    // Same `Arc` shared with the gate and ToolContext - mutating
    // here on Shift+Tab updates every reader atomically.
    app.auto_apply = auto_apply;
    // Surface NO_COLOR through the renderer. Spec: any non-empty value
    // disables color; the variable being unset (or empty) keeps colors.
    // https://no-color.org
    render::set_no_color(std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty()));

    let mut events = EventStream::new();

    // Spinner redraw timer. Ticks at ~10fps; we only advance the
    // spinner counter and redraw when streaming, so idle UI doesn't
    // burn CPU. Discard the immediate first tick.
    let mut spinner_timer = tokio::time::interval(Duration::from_millis(100));
    spinner_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    spinner_timer.tick().await;

    terminal
        .draw(|f| render::render(f, &mut app))
        .context("initial render")?;

    loop {
        tokio::select! {
            ev = events.next() => {
                match ev {
                    Some(Ok(ev)) => {
                        if input::handle_event(&ev, &mut app) == Action::Quit {
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::warn!(error = %e, "crossterm event error");
                    }
                    None => break,
                }
            }
            msg = rx.recv() => {
                match msg {
                    Some(first) => {
                        // Drain bursts (mostly AssistantText token
                        // chunks) into a single post-drain render -
                        // but force an intra-drain render right
                        // after a ToolCallStart so the dynamic
                        // spinner label ("Reading X…") gets at
                        // least one frame even for fast tools where
                        // Start + End arrive in the same drain tick.
                        // Without this, sub-frame tools would skip
                        // the dynamic label entirely (active_tool
                        // is set then cleared before any draw).
                        let force = is_tool_start(&first);
                        app.apply_ui_msg(first);
                        if force {
                            terminal
                                .draw(|f| render::render(f, &mut app))
                                .context("render after tool-start")?;
                        }
                        while let Ok(more) = rx.try_recv() {
                            let force = is_tool_start(&more);
                            app.apply_ui_msg(more);
                            if force {
                                terminal
                                    .draw(|f| render::render(f, &mut app))
                                    .context("render after tool-start")?;
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = spinner_timer.tick(), if matches!(app.mode, app::AppMode::Streaming) => {
                app.render.spinner_tick = app.render.spinner_tick.wrapping_add(1);
            }
            // Auto-clear the armed double-tap chord state (and its
            // "press X again" hint) once ARM_TIMEOUT elapses, even
            // if the user presses nothing. Without this branch the
            // hint would stay visible until the next keystroke
            // forces a re-render. The conditional keeps the branch
            // dormant when nothing is armed.
            () = async {
                match app.arm_state.as_ref() {
                    Some(s) => tokio::time::sleep(app::ARM_TIMEOUT.saturating_sub(s.at.elapsed())).await,
                    None => std::future::pending::<()>().await,
                }
            }, if app.arm_state.is_some() => {
                app.arm_state = None;
            }
        }

        terminal
            .draw(|f| render::render(f, &mut app))
            .context("render")?;

        // After the diff is flushed, the backend's stdout is in a
        // clean state. Push any pending OSC 52 directly so the system
        // clipboard reflects the user's last selection. Empty / pure-
        // whitespace selections are skipped so accidental drags into
        // padding don't fill the clipboard with blank lines.
        if let Some(text) = app.render.clipboard_pending.take() {
            if text.trim().is_empty() {
                tracing::debug!("clipboard_pending is whitespace-only; skipping OSC 52");
            } else {
                tracing::debug!(bytes = text.len(), "writing OSC 52 clipboard sequence");
                let mut out = stdout();
                if let Err(e) = clipboard::write_osc52(&mut out, &text) {
                    tracing::warn!(error = %e, "OSC 52 clipboard write failed");
                }
                let _ = out.flush();
            }
        }
    }

    Ok(())
}

/// `true` when the message is a `ToolCallStart` agent event. Used
/// by the drain loop to interleave a render between Start and the
/// rest of the queued messages, so the dynamic streaming-label
/// has at least one rendered frame to land in.
fn is_tool_start(msg: &UiMsg) -> bool {
    matches!(msg, UiMsg::Agent(AgentEvent::ToolCallStart { .. }))
}

/// Seed a documented config template on first launch. When the
/// resolved config path doesn't exist yet, materialize the
/// current defaults to disk with header + per-field comments so
/// users who prefer editing the file in their editor get a
/// complete reference. Subsequent edits (`/settings`, `/model`)
/// use surgical `toml_edit` writes that preserve user-added
/// comments and reordering. Write failure is non-fatal: we log
/// and proceed with in-memory defaults.
fn seed_config_template_if_missing(cfg_path: Option<&std::path::Path>, cfg: &Config) {
    let Some(path) = cfg_path else { return };
    if path.exists() {
        return;
    }
    if let Err(e) = Config::write_template_to(path, cfg) {
        tracing::warn!(
            error = %e,
            path = %path.display(),
            "couldn't seed config template; continuing without a config file",
        );
    }
}

/// Wire up the provider, session, tools, and agent. The `ui_tx`
/// channel is shared with the [`TuiApprovalGate`] so tool-side
/// approval requests reach the render loop. Returns the
/// `Arc<AtomicU8>` policy handle alongside the agent so the caller
/// can hand it to `AppState` for the Shift+Tab toggle to mutate.
async fn build_agent(
    cfg: &Config,
    cwd: PathBuf,
    ui_tx: tokio::sync::mpsc::UnboundedSender<UiMsg>,
) -> Result<(Arc<Mutex<Agent>>, Arc<std::sync::atomic::AtomicU8>)> {
    let provider = Arc::new(
        HttpProvider::new(
            cfg.provider.base_url.clone(),
            // Config uses "" as the unset sentinel; the provider takes
            // Option so it can skip the bearer-auth header entirely.
            (!cfg.provider.api_key.is_empty()).then(|| cfg.provider.api_key.clone()),
        )
        .context("init HTTP provider")?,
    );
    let session = Session::create(&cfg.paths.data_dir)
        .await
        .with_context(|| format!("create session under {}", cfg.paths.data_dir.display()))?;
    let tools = ToolRegistry::with_builtins();
    // Construct the shared atomic explicitly so we can hand the
    // same `Arc` to both the gate (which reads it to auto-accept
    // file edits under `Safe`) and back to the UI (which mutates
    // it on Shift+Tab). `ToolContext::with_policy` would create
    // its own atomic; building the field directly keeps them wired.
    let auto_apply = Arc::new(std::sync::atomic::AtomicU8::new(cfg.auto_apply.as_u8()));
    let gate: Arc<dyn lumen_core::ApprovalGate> =
        Arc::new(TuiApprovalGate::new(ui_tx, auto_apply.clone()));
    let ctx = ToolContext {
        cwd,
        auto_apply: auto_apply.clone(),
        gate,
    };
    let opts = AgentOptions {
        model: cfg.provider.model.clone(),
        ..AgentOptions::default()
    };
    let mut agent = Agent::new(provider, tools, session, ctx, opts);
    // Seed the framework system prompt so every turn ships a stable
    // byte-identical prefix to the provider (the prompt-cache target).
    // No-op on resumed sessions which already carry their own prompt.
    agent
        .seed_system_prompt(CORE_SYSTEM_PROMPT)
        .await
        .context("seed system prompt")?;
    Ok((Arc::new(Mutex::new(agent)), auto_apply))
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).context("enter alt screen")?;
    // Opt into the Kitty Keyboard Protocol where supported. This is
    // what makes Shift+Enter (and other shifted Ctrl combos) arrive
    // distinguishable from their unshifted equivalents - the historic
    // xterm encoding can't carry Shift on Ctrl+Letter or on Enter.
    // Terminals without support (Terminal.app, default xterm, default
    // gnome-terminal, mintty) silently ignore the request - additive,
    // best-effort, no error.
    //
    // We deliberately do NOT push `REPORT_ALL_KEYS_AS_ESCAPE_CODES`:
    // empirically (kitty as of late 2025) that flag doesn't reliably
    // override terminal-side chord handling for things like
    // `Ctrl+Shift+C`, and where it does it also breaks the terminal's
    // own chords like new-tab. Drag-select auto-copy
    // (`ui.auto_copy_on_select`, default `true`) is the path that
    // works without terminal-config gymnastics; `Alt+C` is the
    // explicit-chord fallback.
    let _ = execute!(
        out,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    // Mouse capture: needed for wheel scroll AND for our custom selection
    // (we receive click/drag/release events to drive the in-app
    // selection-and-copy flow rather than relying on the terminal's
    // native selection, which is intercepted whenever capture is on).
    execute!(out, EnableMouseCapture).context("enable mouse capture")?;
    // Bracketed paste: terminal wraps pasted text in `\x1b[200~ ... \x1b[201~`
    // so a multi-line paste arrives as one Event::Paste(String) instead of
    // a stream of fake Enter keystrokes that would submit on the first \n.
    // Universal across modern terminals; older ones ignore the escape.
    execute!(out, EnableBracketedPaste).context("enable bracketed paste")?;
    Terminal::new(CrosstermBackend::new(out)).context("init terminal")
}

/// RAII guard that restores the terminal to its prior state when dropped.
//
// Critical: a panic inside the TUI loop would otherwise leave the user
// staring at a corrupted, raw-mode-stuck terminal.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // Reverse setup order: stop reporting mouse events, pop the
        // keyboard enhancement flags, drop raw mode, leave alt screen.
        // No-ops on terminals that didn't accept the corresponding
        // setup commands. `execute!` accepts varargs so we batch the
        // stdout-targeted ones into one handle.
        let mut out = stdout();
        let _ = execute!(
            out,
            DisableBracketedPaste,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
