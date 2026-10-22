use super::*;
use std::sync::{Arc, Mutex};

const ORIGINAL: [&[u8]; 3] = [
    b"user.name\nAuthor\0commit.gpgsign\nfalse\0",
    b"Author <author@example.invalid> 1 +0000\n",
    b"Committer <committer@example.invalid> 1 +0000\n",
];

fn scripted(after: [&'static [u8]; 3]) -> Git {
    let mut replies = ORIGINAL.into_iter().chain(after).enumerate();
    Git::new("unobserved-signing-directory", 1.0).with_hook(Arc::new(Mutex::new(
        move |_: &Git, args: &[&str], _: Option<&[u8]>, _: Option<&std::path::Path>| {
            let (step, reply) = replies.next().expect("unexpected extra observation");
            match step % 3 {
                0 => assert!(matches!(args, ["config", "--null", "--get-regexp", _])),
                1 => assert_eq!(args, ["var", "GIT_AUTHOR_IDENT"]),
                2 => assert_eq!(args, ["var", "GIT_COMMITTER_IDENT"]),
                _ => unreachable!(),
            }
            Some(Ok(reply.to_vec()))
        },
    )))
}

#[test]
fn unchanged_profile_accepts_different_identity_timestamps_and_offsets() {
    let git = scripted([
        ORIGINAL[0],
        b"Author <author@example.invalid> 2 +1100\n",
        b"Committer <committer@example.invalid> 999 -0500\n",
    ]);
    let profile = SigningProfile::observe(&git).unwrap();
    assert_eq!(profile.author, "Author <author@example.invalid>");
    assert_eq!(profile.committer, "Committer <committer@example.invalid>");
    profile.verify(&git).unwrap();
}

#[test]
fn changed_author_committer_or_signing_policy_replies_are_rejected() {
    for (field, reply) in [
        (
            0,
            b"user.name\nAuthor\0commit.gpgsign\nfalse\0gpg.program\ndifferent-program\0"
                .as_slice(),
        ),
        (
            1,
            b"Changed Author <author@example.invalid> 1 +0000\n".as_slice(),
        ),
        (
            2,
            b"Changed Committer <committer@example.invalid> 1 +0000\n".as_slice(),
        ),
    ] {
        let mut after = ORIGINAL;
        after[field] = reply;
        let git = scripted(after);
        let prepared = SigningProfile::observe(&git).unwrap();
        assert_eq!(
            prepared.verify(&git).unwrap_err().to_string(),
            "Git identity or signing policy differs at the final catalog path; inspect save.json"
        );
    }
}
