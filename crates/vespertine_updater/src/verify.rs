//! Minisign verification of a complete in-memory package.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use minisign_verify::{Error as MinisignError, PublicKey, Signature};

use crate::error::{Error, Result};

fn decode_text(what: &str, base64_text: &str) -> Result<String> {
    let bytes = STANDARD
        .decode(base64_text.trim())
        .map_err(|error| Error::Verification(format!("the {what} is not valid base64: {error}")))?;
    String::from_utf8(bytes)
        .map_err(|_| Error::Verification(format!("the {what} is not valid UTF-8 text")))
}

/// Verifies `package` against a Minisign signature with an Ed25519 public key. Both are the
/// base64 text of the respective Minisign file contents. Legacy and prehashed signatures are
/// accepted.
pub fn verify_package(package: &[u8], public_key: &str, signature: &str) -> Result<()> {
    let key_text = decode_text("public key", public_key)?;
    let signature_text = decode_text("signature", signature)?;
    let key = PublicKey::decode(&key_text)
        .map_err(|error| Error::Verification(format!("malformed public key: {error}")))?;
    let signature = Signature::decode(&signature_text)
        .map_err(|error| Error::Verification(format!("malformed signature: {error}")))?;
    key.verify(package, &signature, true)
        .map_err(|error| match error {
            MinisignError::UnexpectedKeyId => Error::Verification(
                "the signature was made with a different key than the public key (key id mismatch)"
                    .to_string(),
            ),
            MinisignError::InvalidSignature => {
                Error::Verification("the signature does not match the package".to_string())
            }
            other => Error::Verification(other.to_string()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Produced by the Minisign reference implementation (fixtures of `minisign-verify`):
    // the public key, a legacy signature and a prehashed signature of the bytes `test`.
    const KEY_FILE: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n";
    const LEGACY_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n";
    const PREHASHED_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n";

    fn encode(text: &str) -> String {
        STANDARD.encode(text)
    }

    #[test]
    fn legacy_and_prehashed_signatures_of_the_reference_implementation_verify() {
        let key = encode(KEY_FILE);
        verify_package(b"test", &key, &encode(LEGACY_SIGNATURE)).unwrap();
        verify_package(b"test", &key, &encode(PREHASHED_SIGNATURE)).unwrap();
        // Surrounding whitespace of the base64 text (a trailing newline) is tolerated.
        verify_package(
            b"test",
            &format!("{key}\n"),
            &format!(" {}\n", encode(PREHASHED_SIGNATURE)),
        )
        .unwrap();
    }

    #[test]
    fn changed_bytes_fail_both_signature_kinds() {
        let key = encode(KEY_FILE);
        for signature in [LEGACY_SIGNATURE, PREHASHED_SIGNATURE] {
            let error = verify_package(b"Test", &key, &encode(signature)).unwrap_err();
            assert!(
                matches!(&error, Error::Verification(message) if message.contains("does not match")),
                "{error}"
            );
        }
    }

    #[test]
    fn malformed_inputs_are_verification_errors() {
        let key = encode(KEY_FILE);
        let signature = encode(PREHASHED_SIGNATURE);
        for (public_key, signature) in [
            ("!!", signature.as_str()),
            (key.as_str(), "!!"),
            ("", signature.as_str()),
            (key.as_str(), ""),
            (key.as_str(), &encode("a\nb\nc\nd")),
        ] {
            assert!(
                matches!(
                    verify_package(b"test", public_key, signature),
                    Err(Error::Verification(_))
                ),
                "{public_key:?} {signature:?}"
            );
        }
    }
}
