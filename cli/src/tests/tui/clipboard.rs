use super::*;

fn b64(input: &str) -> String {
    let mut s = String::new();
    base64_encode_into(&mut s, input.as_bytes());
    s
}

#[test]
fn base64_known_vectors() {
    // RFC 4648 vectors.
    assert_eq!(b64(""), "");
    assert_eq!(b64("f"), "Zg==");
    assert_eq!(b64("fo"), "Zm8=");
    assert_eq!(b64("foo"), "Zm9v");
    assert_eq!(b64("foob"), "Zm9vYg==");
    assert_eq!(b64("fooba"), "Zm9vYmE=");
    assert_eq!(b64("foobar"), "Zm9vYmFy");
}

#[test]
fn base64_handles_utf8() {
    // "héllo" -> bytes [68, c3, a9, 6c, 6c, 6f]. Standard base64.
    assert_eq!(b64("héllo"), "aMOpbGxv");
}

#[test]
fn osc52_wraps_with_correct_envelope() {
    let mut buf = Vec::new();
    write_osc52(&mut buf, "hi").unwrap();
    let s = String::from_utf8(buf).unwrap();
    assert!(s.starts_with("\x1b]52;c;"), "missing OSC prefix");
    assert!(s.ends_with("\x1b\\"), "missing ST terminator");
    assert!(s.contains(&b64("hi")));
}
