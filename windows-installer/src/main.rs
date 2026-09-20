//! APEX portable Windows installer — survey and inspection.
//!
//! Nothing in this binary writes to a disk or to a firmware variable. It opens
//! handles for reading only; there is no code path from any of these commands
//! to a write, and that is checked by `tests/test-windows-installer.sh` rather
//! than asserted here.

use apex_windows_installer::{Scan, enumerate, lab_policy, open_image, scan};
use std::{env, io, path::Path, process::ExitCode};

const USAGE: &str = "\
apex-windows-installer -- APEX installer for Windows (survey stage)

  apex-windows-installer survey        every disk, its identity, its partitions
  apex-windows-installer inspect GUID  one partition by its GPT GUID, including
                                       reading every byte of it
  apex-windows-installer lab FILE.img  the offline image laboratory

This build opens disks read-only: installation and firmware changes are disabled.
No partition, no file and no firmware variable is written by any command above.";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> =
        env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("lab") if args.len() == 2 => lab(&args[1]),
        Some("survey") if args.len() == 1 => survey(),
        Some("inspect") if args.len() == 2 => inspect(&args[1]),
        _ => Err(USAGE.into()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  The offline laboratory, unchanged: it is what proves the GPT reader and the
//  content scanner against fixtures that can be built anywhere, including on
//  the Linux machine this is developed on.
// ─────────────────────────────────────────────────────────────────────────────
fn lab(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut image = open_image(Path::new(path))?;
    let original_len = image.metadata()?.len();
    let layout = enumerate(&mut image)?;
    println!(
        "READ-ONLY IMAGE LAB -- no installation available\nDisk GPT GUID: {}\nModel/serial: unavailable (image fixture, not hardware)",
        layout.disk_id
    );
    for p in &layout.partitions {
        println!(
            "{} | {} bytes | GPT name {:?} | filesystem label: not probed | type {} | offset {} | attributes {:#x}",
            p.id, p.length, p.name, p.kind, p.offset, p.attributes
        );
    }
    println!("Select a partition by typing its full GPT GUID (blank cancels):");
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let selected = layout
        .partitions
        .iter()
        .find(|p| p.id == selection.trim())
        .ok_or("no exact partition selected; refusing")?;
    lab_policy(selected)?;
    println!(
        "Checks passed: both GPT CRCs, matching tables and table CRC, bounds, unique GUIDs, no overlaps, Linux type, zero attributes.\nScanning all {} bytes; this may take a long time.",
        selected.length
    );
    let result = scan(&mut image, selected.offset, selected.length)?;
    if image.metadata()?.len() != original_len || enumerate(&mut image)? != layout {
        return Err("image layout changed during inspection".into());
    }
    match result {
        Scan::AllZero { bytes_read } => println!(
            "ALL-ZERO CONTENT: {bytes_read}/{bytes_read} bytes read. No nonzero filesystem, encryption, RAID or nested partition-table bytes observed in this extent.\nThis is not installation authorization: ownership, locking and stable hardware identity are NOT verified. No writes performed."
        ),
        Scan::Nonzero { bytes_read, relative_offset, value } => {
            return Err(format!(
                "NOT EMPTY: nonzero byte {value:#04x} at partition offset {relative_offset} (image offset {}); {bytes_read} bytes read. No writes performed.",
                selected.offset + relative_offset
            )
            .into());
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
//  Windows
// ─────────────────────────────────────────────────────────────────────────────
#[cfg(not(windows))]
fn survey() -> Result<(), Box<dyn std::error::Error>> {
    Err("survey needs Windows storage APIs; this build is not for Windows".into())
}
#[cfg(not(windows))]
fn inspect(_: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("inspect needs Windows storage APIs; this build is not for Windows".into())
}

#[cfg(windows)]
mod win {
    use apex_windows_installer::plan::{self, Claim, DiskIdentity, PartitionFacts, Verdict};
    use apex_windows_installer::windows as w;
    use apex_windows_installer::{Scan, enumerate_in, scan};

    /// One disk, with both readings of its partition table and everything
    /// Windows is doing with it.
    pub struct Surveyed {
        pub identity: DiskIdentity,
        pub path: String,
        pub number: u32,
        pub partitions: Vec<PartitionFacts>,
        pub agreement: Result<(), String>,
    }

    /// Reading a disk fails for ordinary reasons — a card reader with no card,
    /// a disk another process has exclusively. Those are REPORTED, never
    /// dropped: a survey that silently loses a disk is a survey that can call
    /// a machine safe when it is not.
    pub fn surveyed() -> (Vec<Surveyed>, Volumes, Vec<String>) {
        let (drives, mut problems) = w::drives();
        let (vols, vproblems) = w::volumes();
        problems.extend(vproblems);
        let mut out = Vec::new();
        for d in drives {
            let number = match w::drive_number(&d.path) {
                Some(n) => n,
                None => {
                    problems.push(format!("{}: cannot determine the disk number", d.path));
                    continue;
                }
            };
            let mut dev = match w::Device::open(&d.path) {
                Ok(v) => v,
                Err(e) => {
                    problems.push(format!("{}: {e}", d.path));
                    continue;
                }
            };
            // Windows' own view.
            let (win_disk_guid, win_parts) = match w::layout(&dev) {
                Ok(v) => v,
                Err(e) => {
                    problems.push(format!("{}: {e}", d.path));
                    continue;
                }
            };
            // This crate's view, read from the bytes on the same handle. The
            // length comes from IOCTL_DISK_GET_LENGTH_INFO because a device
            // handle's metadata length is zero.
            let agreement = match enumerate_in(&mut dev, d.length) {
                Ok(gpt) => {
                    if gpt.disk_id != win_disk_guid {
                        Err(format!(
                            "the disk GUID in the GPT is {} but Windows reports {win_disk_guid}",
                            gpt.disk_id
                        ))
                    } else {
                        plan::cross_check(
                            &gpt.partitions
                                .iter()
                                .map(|p| PartitionFacts {
                                    id: p.id.clone(),
                                    type_guid: p.kind.clone(),
                                    name: p.name.clone(),
                                    offset: p.offset,
                                    length: p.length,
                                    attributes: p.attributes,
                                })
                                .collect::<Vec<_>>(),
                            &facts(&win_parts),
                        )
                    }
                }
                Err(e) => Err(format!("the on-disk GPT could not be read independently: {e}")),
            };
            out.push(Surveyed {
                identity: DiskIdentity {
                    model: d.model.clone(),
                    serial: d.serial.clone(),
                    bus: d.bus.clone(),
                    length: d.length,
                    sector_size: d.bytes_per_sector,
                    gpt_disk_guid: win_disk_guid,
                },
                path: d.path.clone(),
                number,
                partitions: facts(&win_parts),
                agreement,
            });
        }
        (out, Volumes(vols), problems)
    }

    pub struct Volumes(pub Vec<w::Volume>);

    impl Volumes {
        pub fn claims(&self, disk: u32, offset: u64, length: u64) -> Vec<Claim> {
            self.0
                .iter()
                .filter(|v| v.overlaps(disk, offset, length))
                .map(|v| Claim { what: v.describe_use() })
                .collect()
        }
    }

    fn facts(v: &[w::WinPartition]) -> Vec<PartitionFacts> {
        v.iter()
            .map(|p| PartitionFacts {
                id: p.id.clone(),
                type_guid: p.type_guid.clone(),
                name: p.name.clone(),
                offset: p.offset,
                length: p.length,
                attributes: p.attributes,
            })
            .collect()
    }

    pub fn print_survey() -> Result<(), Box<dyn std::error::Error>> {
        let (disks, vols, problems) = surveyed();
        println!("APEX WINDOWS INSTALLER -- READ-ONLY SURVEY. Nothing is written.");
        println!("disks-found: {}", disks.len());
        for d in &disks {
            println!("\nDISK {}", d.identity.gpt_disk_guid);
            println!("  identity   {}", describe(&d.identity));
            println!("  reached by {} (an enumeration artefact, not an identity)", d.path);
            println!("  sector     {} bytes", d.identity.sector_size);
            match &d.agreement {
                Ok(()) => println!(
                    "  agreement  the on-disk GPT and Windows' partition table AGREE ({} partitions)",
                    d.partitions.len()
                ),
                Err(e) => println!("  agreement  DISAGREE -- {e}"),
            }
            for p in &d.partitions {
                let claims = vols.claims(d.number, p.offset, p.length);
                let verdict = plan::assess(p, &claims);
                println!(
                    "  PARTITION {id}\n    {size}  {tname}  name {name:?}  attributes {attr:#x}\n    bytes {start}..{end}",
                    id = p.id,
                    size = plan::human(p.length),
                    tname = plan::type_name(&p.type_guid),
                    name = p.name,
                    attr = p.attributes,
                    start = p.offset,
                    end = p.offset + p.length,
                );
                if claims.is_empty() {
                    println!("    windows-claims: none");
                } else {
                    for c in &claims {
                        println!("    windows-claims: {}", c.what);
                    }
                }
                match verdict {
                    Verdict::ContentCheckAllowed => println!(
                        "    VERDICT: may be content-checked. Run: apex-windows-installer inspect {}",
                        p.id
                    ),
                    Verdict::Refused(r) => {
                        for line in r.to_string().lines() {
                            println!("    {line}");
                        }
                    }
                }
            }
        }
        if !problems.is_empty() {
            println!("\nCOULD NOT BE READ -- these are reported, not ignored:");
            for p in &problems {
                println!("  {p}");
            }
        }
        println!("\nsurvey-complete");
        Ok(())
    }

    fn describe(d: &DiskIdentity) -> String {
        format!(
            "{} / {} / {} bus, {}",
            if d.model.is_empty() { "(no model reported)" } else { &d.model },
            if d.serial.is_empty() { "(no serial reported)" } else { &d.serial },
            d.bus,
            plan::human(d.length)
        )
    }

    pub fn print_inspect(guid: &str) -> Result<(), Box<dyn std::error::Error>> {
        let guid = guid.trim().to_ascii_lowercase();
        let (disks, vols, problems) = surveyed();

        // A GUID is supposed to be unique across the machine. If it is not,
        // that is a cloned disk, and picking either one is guessing.
        let hits: Vec<_> = disks
            .iter()
            .flat_map(|d| d.partitions.iter().map(move |p| (d, p)))
            .filter(|(_, p)| p.id == guid)
            .collect();
        if hits.is_empty() {
            return Err(format!(
                "no partition on this machine has GPT GUID {guid}. {} disk(s) were read; {} could not be.",
                disks.len(),
                problems.len()
            )
            .into());
        }
        if hits.len() > 1 {
            return Err(format!(
                "GPT GUID {guid} appears on {} partitions. A duplicated partition GUID means a cloned disk; refusing to guess which one you meant.",
                hits.len()
            )
            .into());
        }
        let (disk, part) = hits[0];

        println!("INSPECTING {guid} -- READ-ONLY. Nothing is written.");
        println!("disk  {}", describe(&disk.identity));
        println!("disk-guid {}", disk.identity.gpt_disk_guid);
        match &disk.agreement {
            Ok(()) => println!("agreement the on-disk GPT and Windows' partition table AGREE"),
            Err(e) => return Err(e.clone().into()),
        }

        let claims = vols.claims(disk.number, part.offset, part.length);
        match plan::assess(part, &claims) {
            Verdict::Refused(r) => {
                println!("{r}");
                return Err("this partition is not a candidate".into());
            }
            Verdict::ContentCheckAllowed => {}
        }

        // Exclusivity, re-asked rather than remembered.
        //
        // There is deliberately no FSCTL_LOCK_VOLUME here, and the reason is
        // a finding rather than an omission: Windows creates NO VOLUME OBJECT
        // for a Linux-filesystem-type partition. Measured in the lab — the
        // eligible fixture partitions have no volume, no drive letter and
        // nothing to open. And a partition that DOES have a volume has
        // already been refused by `assess` above, because an overlapping
        // volume is a Claim. So a lock call on this path could only ever run
        // on a partition that was already refused: it would be safety code
        // that never executes, which reads as coverage and is worse than
        // none.
        //
        // What this does instead is re-enumerate the volumes NOW, immediately
        // before the content is read, rather than trusting the survey's
        // answer. Between the survey and here a service can rescan, a user
        // can act in Disk Management, or removable media can appear.
        let (fresh, problems) = w::volumes();
        let now: Vec<_> = fresh
            .iter()
            .filter(|v| v.overlaps(disk.number, part.offset, part.length))
            .collect();
        if !now.is_empty() {
            return Err(format!(
                "Windows has taken this partition since the survey: {}",
                now.iter().map(|v| v.describe_use()).collect::<Vec<_>>().join("; ")
            )
            .into());
        }
        if !problems.is_empty() {
            return Err(format!(
                "the volume list could not be read completely, so 'Windows is not using this' cannot be established: {}",
                problems.join("; ")
            )
            .into());
        }
        println!(
            "exclusivity no volume object covers this partition, re-checked against a fresh volume enumeration. Windows has not mounted it and has nothing here to lock."
        );

        println!("scanning  every one of the {} bytes", part.length);
        let mut dev = w::Device::open(&disk.path)?;
        let result = scan(&mut dev, part.offset, part.length)?;

        // Re-read the table afterwards. A layout that changed under the scan
        // means the answer describes bytes that are no longer where they were.
        let dev2 = w::Device::open(&disk.path)?;
        let (guid2, parts2) = w::layout(&dev2)?;
        if guid2 != disk.identity.gpt_disk_guid || facts(&parts2) != disk.partitions {
            return Err(
                "the partition table changed while its contents were being read".into()
            );
        }

        match result {
            Scan::Nonzero { bytes_read, relative_offset, value } => Err(format!(
                "NOT EMPTY: nonzero byte {value:#04x} at partition offset {relative_offset} (disk offset {}); {bytes_read} bytes read. No writes performed.",
                part.offset + relative_offset
            )
            .into()),
            Scan::AllZero { bytes_read } => {
                println!("ALL-ZERO CONTENT: {bytes_read}/{bytes_read} bytes read.");
                println!("\n{}\n", plan::confirmation_text(&disk.identity, part, None, bytes_read));
                println!(
                    "INSTALLATION IS NOT IMPLEMENTED IN THIS BUILD. Nothing was written, no firmware variable was changed, and no confirmation was requested."
                );
                Ok(())
            }
        }
    }
}

#[cfg(windows)]
fn survey() -> Result<(), Box<dyn std::error::Error>> {
    win::print_survey()
}
#[cfg(windows)]
fn inspect(guid: &str) -> Result<(), Box<dyn std::error::Error>> {
    win::print_inspect(guid)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("REFUSED: {e}");
            ExitCode::FAILURE
        }
    }
}
