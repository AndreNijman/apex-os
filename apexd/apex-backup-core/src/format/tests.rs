use super::*;

/// Hinnant's inverse, written out independently so the forward conversion is
/// checked against arithmetic rather than against itself.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A dense sweep, because the sparse table this test started as **did not
/// notice** `(5 * doy + 2) / 153` becoming `(5 * doy + 3) / 153`. Five known
/// instants happened to miss every day the change moves. Every day from 1970
/// to 2100 does not.
#[test]
fn every_day_for_a_hundred_and_thirty_years_converts_to_the_date_it_is() {
    let mut day = 0i64;
    let end = days_from_civil(2100, 1, 2);
    let mut checked = 0usize;
    while day < end {
        let text = stamp((day as u64) * 86_400 * 1000);
        let y: i64 = text[0..4].parse().expect("four digits");
        let m: u32 = text[4..6].parse().expect("two digits");
        let d: u32 = text[6..8].parse().expect("two digits");
        assert_eq!(
            days_from_civil(y, m, d),
            day,
            "day {day} rendered as {text}, which is a different day"
        );
        assert!((1..=12).contains(&m), "{text} has month {m}");
        assert!((1..=31).contains(&d), "{text} has day {d}");
        day += 1;
        checked += 1;
    }
    assert!(checked > 47_000, "the sweep covered only {checked} days");
}

#[test]
fn a_stamp_is_the_instant_it_claims() {
    // Known instants, checked against a table rather than against this
    // function's own arithmetic.
    assert_eq!(stamp(0), "19700101T000000Z");
    assert_eq!(stamp(1_000), "19700101T000001Z");
    // 2026-09-12T01:42:33Z
    assert_eq!(stamp(1_789_177_353_000), "20260912T014233Z");
    // A leap day, which is where a hand-rolled calendar goes wrong.
    assert_eq!(stamp(1_709_164_800_000), "20240229T000000Z");
    // The end of a century that is not a leap year.
    assert_eq!(stamp(4_102_444_800_000), "21000101T000000Z");
}

#[test]
fn an_id_sorts_by_time_because_the_instant_is_first() {
    let early = SnapshotId::new(1_700_000_000_000).expect("makes one");
    let late = SnapshotId::new(1_800_000_000_000).expect("makes one");
    assert!(early < late, "{early} !< {late}");
}

/// The coupling that `SnapshotId::parse` no longer states as a redundant call.
///
/// An id becomes one segment of an R2 object key, and the capability framework
/// refuses a key segment `valid_name` does not accept — so an id this build
/// would accept from storage but could never address again is a snapshot that
/// can be listed and never read.
///
/// **The first version of this test proved nothing**, and a mutation is what
/// said so: it substituted only digits at the digit positions and only hex at
/// the hex positions, so loosening the shape check to stop requiring digits at
/// all left it green — it never offered a character the check was there to
/// refuse. The alphabet below is hostile on purpose, and every character in it
/// after the first eight is one `valid_name` rejects.
#[test]
fn every_id_the_shape_check_accepts_is_addressable() {
    let base = "20260912T014233Z-0badc0de";
    assert!(SnapshotId::parse(base).is_some());

    let alphabet: Vec<char> = "0123456789abcdefTZ-/:%+*. \\~$\u{7f}".chars().collect();
    let mut accepted = 0usize;
    let mut offered = 0usize;
    for position in 0..base.chars().count() {
        for &replacement in &alphabet {
            let mut chars: Vec<char> = base.chars().collect();
            chars[position] = replacement;
            let candidate: String = chars.into_iter().collect();
            offered += 1;
            let Some(id) = SnapshotId::parse(&candidate) else {
                continue;
            };
            accepted += 1;
            assert!(
                apex_secret_core::operation::valid_name(id.as_str()),
                "{id:?} was accepted from storage and is not a name an R2 key \
                 may contain, so it could be listed and never read"
            );
        }
    }
    assert!(offered > 700, "the sweep offered only {offered} candidates");
    assert!(
        accepted > 200,
        "only {accepted} candidates were accepted, so the sweep is mostly \
         measuring the refusal path and not the acceptance path"
    );
}

#[test]
fn an_id_is_a_name_the_capability_framework_will_carry() {
    for ms in [0u64, 1_789_177_353_000, 4_102_444_800_000] {
        let id = SnapshotId::new(ms).expect("makes one");
        assert!(
            apex_secret_core::operation::valid_name(id.as_str()),
            "{id} is not a name an R2 key may contain"
        );
        assert_eq!(SnapshotId::parse(id.as_str()).as_ref(), Some(&id));
    }
}

/// Ids come back from storage, so they are checked rather than trusted.
#[test]
fn an_id_from_storage_that_is_not_one_is_refused() {
    for bad in [
        "",
        "..",
        ".",
        "20260912T014233Z",              // no tail
        "20260912T014233Z-0badc0d",      // tail too short
        "20260912T014233Z-0badc0def",    // tail too long
        "20260912T014233Z-0BADC0DE",     // uppercase hex
        "20260912T014233Z-0badc0dg",     // not hex
        "20260912X014233Z-0badc0de",     // no T
        "20260912T014233A-0badc0de",     // no Z
        "2026091AT014233Z-0badc0de",     // not digits
        "20260912T014233Z-0badc0de/x",   // a second segment
        "../../etc/passwd",
        "head.json",
    ] {
        assert!(SnapshotId::parse(bad).is_none(), "accepted {bad:?}");
    }
}

#[test]
fn the_head_is_written_last_so_an_interrupted_write_is_absent_and_not_corrupt() {
    let head = Head {
        format: FORMAT.to_string(),
        snapshot: "20260912T014233Z-0badc0de".to_string(),
        created_ms: 1,
        recipient: "apexbk1".to_string(),
        ephemeral: "AAAA".to_string(),
        chunk_bytes: CHUNK_BYTES as u32,
        manifest_chunks: 1,
        data_chunks: 2,
        label: "demo".to_string(),
    };
    let objects = head.objects();
    assert_eq!(
        objects,
        vec![
            "data.000000".to_string(),
            "data.000001".to_string(),
            "manifest.000000".to_string(),
            HEAD_OBJECT.to_string(),
        ]
    );
    assert_eq!(objects.last().map(String::as_str), Some(HEAD_OBJECT));
}

#[test]
fn every_object_name_is_addressable_as_an_r2_key_segment() {
    for index in [0u32, 1, 999_999] {
        for stream in [crate::crypto::Stream::Data, crate::crypto::Stream::Manifest] {
            let name = Head::chunk_object(stream, index);
            assert!(
                apex_secret_core::operation::valid_name(&name),
                "{name} is not a valid key segment"
            );
        }
    }
    assert!(apex_secret_core::operation::valid_name(HEAD_OBJECT));
}

#[test]
fn a_head_refuses_a_field_it_does_not_know_rather_than_ignoring_it() {
    let text = r#"{"format":"apex-backup/1","snapshot":"s","created_ms":1,
        "recipient":"r","ephemeral":"AAAA","chunk_bytes":1,"manifest_chunks":1,
        "data_chunks":1,"label":"l","extra":true}"#;
    assert!(serde_json::from_str::<Head>(text).is_err());
}

#[test]
fn an_ephemeral_key_of_the_wrong_length_is_a_named_fault() {
    let mut head = Head {
        format: FORMAT.to_string(),
        snapshot: "s".to_string(),
        created_ms: 1,
        recipient: "r".to_string(),
        ephemeral: data_encoding::BASE64URL_NOPAD.encode(&[7u8; 32]),
        chunk_bytes: 1,
        manifest_chunks: 0,
        data_chunks: 0,
        label: "l".to_string(),
    };
    assert_eq!(head.ephemeral_bytes(), Ok([7u8; 32]));

    head.ephemeral = data_encoding::BASE64URL_NOPAD.encode(&[7u8; 31]);
    assert!(matches!(
        head.ephemeral_bytes(),
        Err(FormatError::BadHead { field: "ephemeral", .. })
    ));

    head.ephemeral = "not base64!".to_string();
    assert!(matches!(
        head.ephemeral_bytes(),
        Err(FormatError::BadHead { field: "ephemeral", .. })
    ));
}

#[test]
fn a_digest_is_the_sha256_a_person_can_check_with_sha256sum() {
    // `printf '' | sha256sum` and `printf 'abc' | sha256sum`.
    assert_eq!(
        digest_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        digest_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_manifest_round_trips_through_json() {
    let manifest = Manifest {
        entries: vec![Entry {
            path: "src/main.rs".to_string(),
            kind: Kind::File,
            size: 3,
            offset: 0,
            mode: 0o644,
            mtime_ms: 17,
            digest: digest_hex(b"abc"),
        }],
        data_bytes: 3,
    };
    let text = serde_json::to_vec(&manifest).expect("serialises");
    assert_eq!(
        serde_json::from_slice::<Manifest>(&text).expect("parses"),
        manifest
    );
}
