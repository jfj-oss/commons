//! Who may be heard and seen (call.js `_reviewPub`): a remote track is subscribed only if its sender is a current
//! member of the room's MLS group (`call_state.members`) and the server marks the publication encrypted. Everything else
//! stays unsubscribed and is listed as blocked, with the reason.

pub const NOT_MEMBER: &str = "not a current member of this room";
pub const NOT_ENCRYPTED: &str = "not marked as encrypted";

/// `None`: may be subscribed. `trust_all` exists only so a test can prove the cryptography alone keeps a removed member out.
pub fn verdict(is_member: bool, encrypted: bool, trust_all: bool) -> Option<&'static str> {
    if !is_member && !trust_all { Some(NOT_MEMBER) } else if !encrypted { Some(NOT_ENCRYPTED) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn members_with_encrypted_tracks_only() {
        assert_eq!(verdict(true, true, false), None);
        assert_eq!(verdict(false, true, false), Some(NOT_MEMBER));
        assert_eq!(verdict(true, false, false), Some(NOT_ENCRYPTED));
        assert_eq!(verdict(false, false, true), Some(NOT_ENCRYPTED), "trusting everyone never lets plain media in");
        assert_eq!(verdict(false, true, true), None);
    }
}
