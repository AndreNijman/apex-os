use apex_windows_installer::{enumerate, lab_policy, open_image, scan, Scan};
use std::{env, io, path::Path, process::ExitCode};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 2 || args[0] != "lab" {
        return Err("usage: apex-windows-installer lab FILE.img\nRead-only image prototype. Windows disks, installation and firmware changes are disabled.".into());
    }
    let mut image = open_image(Path::new(&args[1]))?;
    let original_len = image.metadata()?.len();
    let layout = enumerate(&mut image)?;
    println!("READ-ONLY IMAGE LAB — no installation available\nDisk GPT GUID: {}\nModel/serial: unavailable (image fixture, not hardware)", layout.disk_id);
    for p in &layout.partitions {
        println!("{} | {} bytes | GPT name {:?} | filesystem label: not probed | type {} | offset {} | attributes {:#x}",
            p.id, p.length, p.name, p.kind, p.offset, p.attributes);
    }
    println!("Select a partition by typing its full GPT GUID (blank cancels):");
    let mut selection = String::new();
    io::stdin().read_line(&mut selection)?;
    let selected = layout.partitions.iter().find(|p| p.id == selection.trim())
        .ok_or("no exact partition selected; refusing")?;
    lab_policy(selected)?;
    println!("Checks passed: both GPT CRCs, matching tables and table CRC, bounds, unique GUIDs, no overlaps, Linux type, zero attributes.\nScanning all {} bytes; this may take a long time.", selected.length);
    let result = scan(&mut image, selected.offset, selected.length)?;
    if image.metadata()?.len() != original_len || enumerate(&mut image)? != layout {
        return Err("image layout changed during inspection".into());
    }
    match result {
        Scan::AllZero { bytes_read } => println!("ALL-ZERO CONTENT: {bytes_read}/{bytes_read} bytes read. No nonzero filesystem, encryption, RAID or nested partition-table bytes observed in this extent.\nThis is not installation authorization: ownership, locking and stable hardware identity are NOT verified. No writes performed."),
        Scan::Nonzero { bytes_read, relative_offset, value } => return Err(format!("NOT EMPTY: nonzero byte {value:#04x} at partition offset {relative_offset} (image offset {}); {bytes_read} bytes read. No writes performed.", selected.offset+relative_offset).into()),
    }
    Ok(())
}
fn main() -> ExitCode {
    match run() { Ok(()) => ExitCode::SUCCESS, Err(e) => { eprintln!("REFUSED: {e}"); ExitCode::FAILURE } }
}
