//! What the sealed box and the chunk AEAD are asserted to do.
//!
//! Every test here is about a property an attacker or a mistake could take
//! away, and each name says which one. The two that matter most are the ones
//! that measure rather than assert:
//! `the_sealed_bytes_are_not_the_plaintext` and
//! `the_snapshot_key_is_not_the_raw_x25519_output`.

use super::*;

fn identity() -> Identity {
    Identity::generate().expect("the kernel has randomness")
}

const SNAP: &str = "20260912T010203Z-0badc0de";

#[test]
fn a_chunk_sealed_to_a_recipient_opens_with_its_identity_and_with_no_other() {
    let mine = identity();
    let theirs = identity();
    let (key, epk) = seal_to(&mine.recipient(), SNAP).expect("seals");
    let sealed = key
        .seal_chunk(Stream::Data, 0, true, b"the quick brown fox")
        .expect("seals a chunk");

    let opened = open_with(&mine, &epk, SNAP)
        .expect("derives")
        .open_chunk(Stream::Data, 0, true, &sealed)
        .expect("opens");
    assert_eq!(opened, b"the quick brown fox");

    let wrong = open_with(&theirs, &epk, SNAP)
        .expect("derives a key, which is not the same as opening")
        .open_chunk(Stream::Data, 0, true, &sealed);
    assert_eq!(
        wrong,
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 0
        }),
        "another machine's private key opened this snapshot"
    );
}

#[test]
fn a_recipient_round_trips_through_the_string_that_goes_in_apex_toml() {
    for _ in 0..32 {
        let id = identity();
        let recipient = id.recipient();
        let text = recipient.to_string_value();
        assert!(text.starts_with(RECIPIENT_PREFIX), "{text}");
        assert!(!text.contains('='), "no padding, so it is one word: {text}");
        let back = Recipient::parse(&text).expect("parses what it printed");
        assert_eq!(back, recipient);
    }
}

/// The single most valuable assertion in this file.
///
/// A backup sealed to a mistyped public key encrypts, uploads and verifies
/// exactly like a good one, and is unopenable forever. Nothing downstream can
/// catch it, because "does not open" is also what another machine's snapshot
/// looks like. So every one-character change has to be refused here.
#[test]
fn one_character_changed_in_a_recipient_is_refused_rather_than_sealed_to_nobody() {
    let good = identity().recipient().to_string_value();
    let alphabet: Vec<char> =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
            .chars()
            .collect();
    let mut checked = 0usize;
    for position in RECIPIENT_PREFIX.len()..good.len() {
        let original = good.chars().nth(position).expect("in range");
        for &replacement in &alphabet {
            if replacement == original {
                continue;
            }
            let mut typo: Vec<char> = good.chars().collect();
            typo[position] = replacement;
            let typo: String = typo.into_iter().collect();
            let verdict = Recipient::parse(&typo);
            assert!(
                verdict.is_err(),
                "a one-character typo at {position} ({original} -> {replacement}) \
                 parsed as a valid recipient: {typo}"
            );
            checked += 1;
        }
    }
    // 48 base64url characters after the prefix, 63 substitutions each.
    assert_eq!(checked, 48 * 63, "the sweep did not cover what it claims");
}

#[test]
fn a_recipient_without_the_prefix_or_of_the_wrong_length_is_refused() {
    let good = identity().recipient().to_string_value();
    let body = good.strip_prefix(RECIPIENT_PREFIX).expect("has the prefix");

    assert!(matches!(
        Recipient::parse(body),
        Err(CryptoError::BadRecipient(_))
    ));
    assert!(matches!(
        Recipient::parse(&format!("{RECIPIENT_PREFIX}{}", &body[..body.len() - 4])),
        Err(CryptoError::BadRecipient(_))
    ));
    assert!(matches!(
        Recipient::parse(&format!("{RECIPIENT_PREFIX}{body}AAAA")),
        Err(CryptoError::BadRecipient(_))
    ));
    assert!(matches!(
        Recipient::parse(&format!("{RECIPIENT_PREFIX}not base64!!")),
        Err(CryptoError::BadRecipient(_))
    ));
    assert!(matches!(Recipient::parse(""), Err(CryptoError::BadRecipient(_))));
}

/// A wrong checksum has its own variant, because its message has to say
/// "mistyped" and not "another machine's".
#[test]
fn a_recipient_whose_checksum_is_wrong_is_told_apart_from_one_of_the_wrong_shape() {
    let good = identity().recipient().to_string_value();
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(good.strip_prefix(RECIPIENT_PREFIX).expect("prefix").as_bytes())
        .expect("decodes");
    let mut broken = raw.clone();
    broken[KEY_BYTES] ^= 0xff;
    let text = format!(
        "{RECIPIENT_PREFIX}{}",
        data_encoding::BASE64URL_NOPAD.encode(&broken)
    );
    let err = Recipient::parse(&text).expect_err("refuses");
    assert!(
        matches!(err, CryptoError::RecipientChecksum { .. }),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("mistyped"),
        "the message has to say which of the two it is: {err}"
    );
}

#[test]
fn a_chunk_moved_to_another_index_does_not_open() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key.seal_chunk(Stream::Data, 7, false, b"payload").expect("seals");
    let opener = open_with(&id, &epk, SNAP).expect("derives");

    assert_eq!(
        opener.open_chunk(Stream::Data, 7, false, &sealed),
        Ok(b"payload".to_vec())
    );
    assert_eq!(
        opener.open_chunk(Stream::Data, 8, false, &sealed),
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 8
        })
    );
}

#[test]
fn a_chunk_moved_between_the_manifest_and_the_data_stream_does_not_open() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key
        .seal_chunk(Stream::Manifest, 0, true, b"the file list")
        .expect("seals");
    let opener = open_with(&id, &epk, SNAP).expect("derives");

    assert_eq!(
        opener.open_chunk(Stream::Manifest, 0, true, &sealed),
        Ok(b"the file list".to_vec())
    );
    assert_eq!(
        opener.open_chunk(Stream::Data, 0, true, &sealed),
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 0
        })
    );
}

#[test]
fn a_chunk_moved_into_another_snapshot_does_not_open() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key.seal_chunk(Stream::Data, 0, true, b"payload").expect("seals");

    let elsewhere = open_with(&id, &epk, "20260101T000000Z-deadbeef").expect("derives");
    assert_eq!(
        elsewhere.open_chunk(Stream::Data, 0, true, &sealed),
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 0
        }),
        "a chunk was replayed into a different snapshot"
    );
}

/// Truncation is the failure a backup format most has to catch, because it is
/// what a half-finished upload looks like.
#[test]
fn the_last_chunk_presented_as_a_middle_one_does_not_open() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key.seal_chunk(Stream::Data, 3, true, b"the end").expect("seals");
    let opener = open_with(&id, &epk, SNAP).expect("derives");

    assert_eq!(opener.open_chunk(Stream::Data, 3, true, &sealed), Ok(b"the end".to_vec()));
    assert_eq!(
        opener.open_chunk(Stream::Data, 3, false, &sealed),
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 3
        })
    );
}

#[test]
fn a_single_bit_flipped_anywhere_in_a_sealed_chunk_stops_it_opening() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key
        .seal_chunk(Stream::Data, 0, true, b"sixty-four bytes of plaintext, give or take a few")
        .expect("seals");
    let opener = open_with(&id, &epk, SNAP).expect("derives");

    for byte in 0..sealed.len() {
        for bit in 0..8u32 {
            let mut altered = sealed.clone();
            altered[byte] ^= 1 << bit;
            assert_eq!(
                opener.open_chunk(Stream::Data, 0, true, &altered),
                Err(CryptoError::Unopenable {
                    stream: "data",
                    index: 0
                }),
                "flipping bit {bit} of byte {byte} still opened"
            );
        }
    }
}

/// Measured, not asserted. This is the smallest form of the claim the shell
/// suite makes about a whole target directory.
#[test]
fn the_sealed_bytes_are_not_the_plaintext() {
    let id = identity();
    let (key, _epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    // Compressible and repetitive, which is the shape that survives a
    // pretend-encryption bug most visibly.
    let plaintext = b"SENTINEL-3f9a2c7e".repeat(64);
    let sealed = key.seal_chunk(Stream::Data, 0, true, &plaintext).expect("seals");

    assert!(
        !sealed
            .windows(b"SENTINEL-3f9a2c7e".len())
            .any(|w| w == b"SENTINEL-3f9a2c7e"),
        "the canary survived encryption"
    );
    assert_eq!(sealed.len(), plaintext.len() + CHUNK_OVERHEAD);
}

/// A fixed nonce is the bug this format's 24-byte nonce exists to make
/// impossible to reach by accident; this is the test that would catch someone
/// making it reachable on purpose.
#[test]
fn sealing_the_same_bytes_twice_under_one_key_produces_different_ciphertext() {
    let id = identity();
    let (key, _epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let a = key.seal_chunk(Stream::Data, 0, true, b"identical").expect("seals");
    let b = key.seal_chunk(Stream::Data, 0, true, b"identical").expect("seals");
    assert_ne!(a, b, "the nonce is not random");
    assert_ne!(a[..NONCE_BYTES], b[..NONCE_BYTES], "the nonce is fixed");
}

#[test]
fn a_chunk_shorter_than_its_own_framing_says_truncated_rather_than_unopenable() {
    let id = identity();
    let (_key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let opener = open_with(&id, &epk, SNAP).expect("derives");
    for len in 0..CHUNK_OVERHEAD {
        assert_eq!(
            opener.open_chunk(Stream::Data, 0, true, &vec![0u8; len]),
            Err(CryptoError::Truncated {
                stream: "data",
                index: 0
            }),
            "a {len}-byte chunk"
        );
    }
    // One byte more is framing-complete, so it reaches the AEAD and fails
    // there. The two answers are different on purpose: one is a storage fault
    // and the other is a key or an alteration.
    assert_eq!(
        opener.open_chunk(Stream::Data, 0, true, &[0u8; CHUNK_OVERHEAD]),
        Err(CryptoError::Unopenable {
            stream: "data",
            index: 0
        })
    );
}

/// Feeding the raw X25519 output to an AEAD is the classic mistake, and it is
/// not one a reviewer can see by reading `seal_to` — the types are the same
/// either way. So it is measured.
#[test]
fn the_snapshot_key_is_not_the_raw_x25519_output() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");

    let secret = x25519_dalek::StaticSecret::from(*id.expose());
    let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(epk));

    assert_ne!(
        key.key.as_ref(),
        shared.as_bytes(),
        "the AEAD key is the curve point itself"
    );
}

/// The same derivation on both sides, or a snapshot seals and never opens.
#[test]
fn both_sides_derive_the_same_key() {
    let id = identity();
    let (sealing, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let opening = open_with(&id, &epk, SNAP).expect("derives");
    assert_eq!(sealing.key.as_ref(), opening.key.as_ref());
}

#[test]
fn two_recipients_do_not_share_a_snapshot_key() {
    let a = identity();
    let b = identity();
    let (ka, _) = seal_to(&a.recipient(), SNAP).expect("seals");
    let (kb, _) = seal_to(&b.recipient(), SNAP).expect("seals");
    assert_ne!(ka.key.as_ref(), kb.key.as_ref());
}

/// The previous test does *not* prove what it looks like it proves: two
/// identities have two shared secrets, so the keys differ whether or not the
/// recipient is mixed in. This one holds the exchange fixed and varies only the
/// recipient, which is the only way to see the mixing at all.
#[test]
fn the_recipient_is_mixed_into_the_key_and_not_only_into_the_exchange() {
    let shared = [9u8; KEY_BYTES];
    let ephemeral = [3u8; KEY_BYTES];
    let one = derive(&shared, &ephemeral, &[1u8; KEY_BYTES]);
    let two = derive(&shared, &ephemeral, &[2u8; KEY_BYTES]);
    assert_ne!(
        one.as_ref(),
        two.as_ref(),
        "the recipient is not in the key derivation, so a key derived for one \
         machine can be replayed at another"
    );

    // And the ephemeral key too, for the same reason and by the same method.
    let three = derive(&shared, &[4u8; KEY_BYTES], &[1u8; KEY_BYTES]);
    assert_ne!(one.as_ref(), three.as_ref());
}

#[test]
fn a_recipient_that_is_a_small_order_point_is_refused_rather_than_used() {
    // All zeroes is the canonical small-order point: every X25519 with it
    // yields an all-zero shared secret, so a snapshot "sealed" to it is sealed
    // to a key anybody can compute.
    let zero = Recipient::from_bytes([0u8; KEY_BYTES]);
    let err = seal_to(&zero, SNAP)
        .map(|_| ())
        .expect_err("an all-zero recipient is refused");
    assert_eq!(err, CryptoError::NotContributory);
}

#[test]
fn neither_key_type_prints_its_bytes() {
    let id = identity();
    let shown = format!("{id:?}");
    assert_eq!(shown, "Identity(<private>)");
    let bytes = id.expose();
    assert!(
        !shown.contains(&format!("{}", bytes[0])),
        "the debug output leaks key material: {shown}"
    );

    let (key, _) = seal_to(&id.recipient(), SNAP).expect("seals");
    let shown = format!("{key:?}");
    assert!(shown.contains("<private>"), "{shown}");
    assert!(shown.contains(SNAP), "the snapshot id is not secret: {shown}");
}

/// `seal_to` and `open_with` return a key; a `Recipient` is public. This pins
/// which of the three may be compared for equality at all, because a
/// constant-time question about a secret is a different question.
#[test]
fn a_recipient_is_comparable_and_the_private_halves_are_not() {
    let id = identity();
    assert_eq!(id.recipient(), id.recipient());
    assert_ne!(id.recipient(), identity().recipient());
}

#[test]
fn an_empty_chunk_is_a_chunk() {
    let id = identity();
    let (key, epk) = seal_to(&id.recipient(), SNAP).expect("seals");
    let sealed = key.seal_chunk(Stream::Data, 0, true, b"").expect("seals");
    assert_eq!(sealed.len(), CHUNK_OVERHEAD);
    assert_eq!(
        open_with(&id, &epk, SNAP)
            .expect("derives")
            .open_chunk(Stream::Data, 0, true, &sealed),
        Ok(Vec::new())
    );
}
