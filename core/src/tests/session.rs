use super::*;
use crate::provider::Role;
use tempfile::tempdir;

fn user_msg(text: &str) -> Message {
    Message {
        role: Role::User,
        content: text.into(),
        tool_calls: None,
        tool_call_id: None,
    }
}

#[tokio::test]
async fn ephemeral_session_holds_messages_in_memory() {
    let mut s = Session::ephemeral();
    s.push(user_msg("hi")).await.unwrap();
    s.push(user_msg("hello")).await.unwrap();
    assert_eq!(s.messages().len(), 2);
    assert!(s.transcript_path().is_none());
}

#[tokio::test]
async fn create_writes_jsonl_per_event() {
    let dir = tempdir().unwrap();
    let mut s = Session::create(dir.path()).await.unwrap();
    s.note("session start").await.unwrap();
    s.push(user_msg("hi")).await.unwrap();

    let path = s.transcript_path().unwrap().to_path_buf();
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    let lines: Vec<&str> = on_disk.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(r#""kind":"note""#));
    assert!(lines[0].contains(r#""text":"session start""#));
    assert!(lines[1].contains(r#""kind":"message""#));
    assert!(lines[1].contains(r#""content":"hi""#));
}

#[tokio::test]
async fn resume_replays_messages_and_appends() {
    let dir = tempdir().unwrap();
    let id;
    let path;
    {
        let mut s = Session::create(dir.path()).await.unwrap();
        s.push(user_msg("first")).await.unwrap();
        s.push(user_msg("second")).await.unwrap();
        id = s.id();
        path = s.transcript_path().unwrap().to_path_buf();
    }
    let mut s = Session::resume(&path).await.unwrap();
    assert_eq!(s.id(), id);
    assert_eq!(s.messages().len(), 2);
    assert_eq!(s.messages()[0].content, "first");

    s.push(user_msg("third")).await.unwrap();
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(on_disk.lines().count(), 3);
}

#[tokio::test]
async fn resume_skips_non_message_events_in_history() {
    let dir = tempdir().unwrap();
    let path;
    {
        let mut s = Session::create(dir.path()).await.unwrap();
        s.note("ignore me").await.unwrap();
        s.push(user_msg("real msg")).await.unwrap();
        s.note("ignore me too").await.unwrap();
        path = s.transcript_path().unwrap().to_path_buf();
    }
    let s = Session::resume(&path).await.unwrap();
    assert_eq!(s.messages().len(), 1);
    assert_eq!(s.messages()[0].content, "real msg");
}

#[tokio::test]
async fn invalid_filename_fails_resume() {
    let dir = tempdir().unwrap();
    let bad = dir.path().join("not-a-uuid.jsonl");
    tokio::fs::write(&bad, "").await.unwrap();
    let err = Session::resume(&bad).await.unwrap_err();
    assert!(matches!(err, Error::Tool(_)));
}

#[tokio::test]
async fn reset_to_system_prompt_leaves_single_system_message() {
    let mut s = Session::ephemeral();
    s.push(user_msg("hi")).await.unwrap();
    s.push(user_msg("again")).await.unwrap();
    assert_eq!(s.messages().len(), 2);

    s.reset_to_system_prompt("you are lumen");
    assert_eq!(s.messages().len(), 1);
    assert_eq!(s.messages()[0].role, Role::System);
    assert_eq!(s.messages()[0].content, "you are lumen");
}

#[tokio::test]
async fn reset_to_system_prompt_does_not_touch_transcript() {
    // The user-visible /clear keeps the on-disk transcript as a
    // permanent record of what was said before; only the in-memory
    // replay window for the next provider call is rewritten.
    let dir = tempdir().unwrap();
    let path;
    {
        let mut s = Session::create(dir.path()).await.unwrap();
        s.push(user_msg("turn one")).await.unwrap();
        s.push(user_msg("turn two")).await.unwrap();
        s.reset_to_system_prompt("you are lumen");
        path = s.transcript_path().unwrap().to_path_buf();
    }
    let on_disk = tokio::fs::read_to_string(&path).await.unwrap();
    // Two pre-clear messages persisted; the reset added no transcript line.
    assert_eq!(on_disk.lines().count(), 2);
    assert!(on_disk.contains("turn one"));
    assert!(on_disk.contains("turn two"));
}
