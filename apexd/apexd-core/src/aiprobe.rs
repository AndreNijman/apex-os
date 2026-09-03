//! Reading the machine, so [`crate::ai`] can decide about it (roadmap §14).
//!
//! Every function here is the I/O half of a pure planner in that module: this
//! file gathers evidence and never chooses. It is the same arrangement
//! [`crate::fingerprint`] and [`crate::select`] use, and it is why [`crate::ai`]
//! contains no `std::fs` call at all — the split is what lets every rule about
//! backend selection, VRAM and idle unloading be unit-tested with no GPU.
//!
//! It lives in `apexd-core` rather than in the daemon because **two** programs
//! need it. `apex-aid` probes to decide what to launch; `apex ai status` probes
//! to report what *would* be launched on a machine where the daemon is not even
//! enabled yet — which is the first command anyone runs. Two copies of this
//! would be two answers to "which backend will this machine use", and the one
//! the user saw would not be the one that ran.
//!
//! Nothing here spawns a process. `nvidia-smi` is reached through the injected
//! [`NvidiaSmi`] trait, so a test supplies a mock and the static CI check that
//! keeps §14's planners spawn-free covers this file too.
//!
//! ── Why the roots are overridable ──────────────────────────────────────────
//!
//! `APEX_AI_ROOT` prefixes every path read here. It exists for the same reason
//! `APEX_SYS_ROOT` does in the Performance Lab: the shell suite has to be able
//! to describe a machine with a CUDA device, a machine with only lavapipe, and
//! a machine with nothing, and it must do that without any of those answers
//! depending on which laptop is running the tests.
//!
//! It is read once, at startup, and it moves only *reads*. Nothing the daemon
//! writes is affected by it, and the model store has its own variable —
//! `APEX_AI_STORE` — because a redirected store is a much bigger claim than a
//! redirected `/sys` and the two must not be conflated.

use std::path::{Path, PathBuf};

use crate::ai::{
    plan_fit, plan_launch, probe_paths, select_backend, Accel, AccelEvidence, BackendChoice,
    Device, Endpoints, Fit, LaunchPlan, LaunchRequest, Listen, Manifest, ModelInfo, Runtime,
    Settings, Store, MIN_USEFUL_CONTEXT,
};
use crate::gpu::NvidiaSmi;

/// Filesystem roots every read goes through.
///
/// The default — an empty prefix — is the live machine, so a caller that forgets
/// to build one reads the real `/sys` and `/dev` rather than an empty fixture
/// that would make every accelerator look absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    /// Prefix applied to every absolute path below. Empty on a live machine.
    pub prefix: PathBuf,
}

impl Roots {
    /// The live machine, or whatever `APEX_AI_ROOT` names.
    pub fn from_env() -> Roots {
        match std::env::var_os("APEX_AI_ROOT") {
            Some(p) if !p.is_empty() => Roots { prefix: PathBuf::from(p) },
            _ => Roots::default(),
        }
    }

    /// Resolve an absolute path against the prefix.
    ///
    /// `Path::join` on an absolute argument *replaces* the base, which would
    /// silently make every read hit the real machine — the exact bug that makes
    /// a fixture-based suite pass while testing the developer's laptop. So the
    /// leading `/` is stripped first.
    pub fn at(&self, abs: &str) -> PathBuf {
        if self.prefix.as_os_str().is_empty() {
            return PathBuf::from(abs);
        }
        self.prefix.join(abs.trim_start_matches('/'))
    }
}

/// The model store, from `APEX_AI_STORE` or the shipped default.
///
/// Separate from [`Roots`] on purpose: `APEX_AI_ROOT` redirects measurements,
/// which is harmless, while redirecting the store changes which weights get
/// loaded. Two names so a suite cannot get the second by asking for the first.
pub fn store_from_env() -> Store {
    match std::env::var_os("APEX_AI_STORE") {
        Some(p) if !p.is_empty() => Store::new(Path::new(&p)),
        _ => Store::default(),
    }
}

/// Gather the accelerator evidence.
///
/// Reads exactly the four paths named in [`probe_paths`] and nothing else, so
/// what the classifier sees is what the constants say it sees.
pub fn accel_evidence(roots: &Roots) -> AccelEvidence {
    let mut icds: Vec<String> = std::fs::read_dir(roots.at(probe_paths::VULKAN_ICD_DIR))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    // Sorted, so the reported ICD list — and therefore `apex ai status` — is
    // the same on two identical machines. readdir order is not.
    icds.sort();

    let render_nodes = std::fs::read_dir(roots.at(probe_paths::DRI_DIR))
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|n| n.starts_with("renderD"))
                })
                .count() as u32
        })
        .unwrap_or(0);

    AccelEvidence {
        nvidia_control_dev: roots.at(probe_paths::NVIDIA_CONTROL).exists(),
        libcuda: roots.at(probe_paths::LIBCUDA).exists(),
        kfd_dev: roots.at(probe_paths::KFD).exists(),
        render_nodes,
        vulkan_icds: icds,
    }
}

/// Which backends this machine can run.
pub fn accel(roots: &Roots) -> Accel {
    accel_evidence(roots).accel()
}

/// Every accelerator with a VRAM reading, NVIDIA first.
///
/// Two sources, because no single one covers the hardware APEX ships on:
///
/// * `nvidia-smi`, behind the injected [`NvidiaSmi`] trait, because the
///   proprietary driver exposes no VRAM total in sysfs at all. Reusing the
///   trait rather than spawning here is what lets a test supply a mock.
/// * `amdgpu`'s `mem_info_vram_{used,total}` in sysfs, which is the only
///   portable-shaped VRAM interface a kernel driver offers. `i915`/`xe` publish
///   no total, so an Intel iGPU appears with no device entry rather than with a
///   wrong one — and [`crate::ai::select_backend`] then reports "no device
///   reported its VRAM" instead of planning against a guess.
pub fn devices(roots: &Roots, smi: &dyn NvidiaSmi) -> Vec<Device> {
    let mut out = Vec::new();

    // NVIDIA. `query` supplies names, `vram_mib` the memory; they are separate
    // calls in the trait because they have different lifetimes.
    let names: Vec<(u32, String)> = smi.query().into_iter().map(|g| (g.index, g.name)).collect();
    for (index, used_mib, total_mib) in smi.vram_mib() {
        let name = names
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| format!("NVIDIA GPU {index}"));
        out.push(Device { index, name, total_mib, used_mib });
    }

    // amdgpu. Numbered after the NVIDIA devices so an index is stable within
    // one probe; a machine with both is rare and the alternative — renumbering
    // NVIDIA — would break `--main-gpu`.
    let drm = roots.at("/sys/class/drm");
    let mut cards: Vec<PathBuf> = std::fs::read_dir(&drm)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|s| s.to_str())
                        // `card1`, not `card1-eDP-1`: a connector carries no memory.
                        .is_some_and(|s| s.starts_with("card") && !s.contains('-'))
                })
                .collect()
        })
        .unwrap_or_default();
    cards.sort();
    for card in cards {
        let dev = card.join("device");
        let used = read_u64(&dev.join("mem_info_vram_used"));
        let total = read_u64(&dev.join("mem_info_vram_total"));
        if let (Some(used), Some(total)) = (used, total) {
            let name = std::fs::read_to_string(dev.join("product_name"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    card.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("drm card")
                        .to_string()
                });
            out.push(Device {
                index: out.len() as u32,
                name,
                // sysfs reports bytes; the planner works in MiB.
                total_mib: total / (1024 * 1024),
                used_mib: used / (1024 * 1024),
            });
        }
    }

    out
}

/// Whether the machine is running on battery.
///
/// True only when a mains supply is *found and offline*. A desktop has no
/// `Mains` entry at all, and reading that as "on battery" would give every
/// desktop the short battery idle timeout — so the absence of evidence is
/// treated as AC, which is what a machine with no battery is.
pub fn on_battery(roots: &Roots) -> bool {
    let base = roots.at("/sys/class/power_supply");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return false;
    };
    let mut saw_mains = false;
    for e in entries.flatten() {
        let p = e.path();
        let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        if kind.trim() != "Mains" {
            continue;
        }
        saw_mains = true;
        if read_u64(&p.join("online")) == Some(1) {
            return false;
        }
    }
    saw_mains
}

/// Every model in the store, with whether its blob is actually there.
///
/// A manifest whose blob is missing is what a half-finished `apex ai rm` or a
/// hand-deleted file leaves behind. It is reported rather than hidden, because
/// "the model is listed and will not load" is worse than either alternative.
///
/// A manifest that fails validation is skipped with a warning rather than
/// failing the listing: one bad file must not make `apex ai models` refuse to
/// print the others.
pub fn installed(store: &Store) -> Vec<(Manifest, bool)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(store.manifests_dir()) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match Manifest::parse(&text) {
            Ok(m) => {
                let present = store.blob(&m.digest).map(|b| b.exists()).unwrap_or(false);
                out.push((m, present));
            }
            Err(e) => eprintln!("apex: ignoring {}: {e}", path.display()),
        }
    }
    out
}

/// Render the store as the control protocol reports it.
pub fn model_infos(
    store: &Store,
    selected: Option<&str>,
    loaded: Option<&str>,
) -> Vec<ModelInfo> {
    installed(store)
        .into_iter()
        .map(|(m, present)| ModelInfo {
            selected: selected == Some(m.id.as_str()),
            loaded: loaded == Some(m.id.as_str()),
            id: m.id,
            digest: m.digest,
            weights_mib: m.weights_mib,
            runtime: m.runtime,
            max_context: m.max_context,
            present,
            user_supplied_digest: m.user_supplied_digest,
        })
        .collect()
}

/// Where a runtime was found, and which one it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// Which runtime.
    pub runtime: Runtime,
    /// The program to execute.
    pub path: PathBuf,
}

/// Find an inference runtime.
///
/// Search order, and every step is a mechanism that already exists:
///
/// 1. `$APEX_AI_RUNTIME` — an explicit override. It is what the shell suite
///    points at a fake backend, and what a developer points at a build tree.
/// 2. `/usr/bin/<program>` — where a system extension lands, so
///    `sudo apex install llama-cpp` is all it takes.
/// 3. `PATH` — for a runtime reached some other way.
///
/// A capsule runtime (`apex env create cuda`) is deliberately **not** searched.
/// Spawning into a capsule would mean the store and the socket directory being
/// visible inside the container and the GPU being passed through, and neither
/// claim could be verified on this branch without downloading a multi-gigabyte
/// CUDA image. So `apex ai status` names the command and stops there rather
/// than shipping a spawn path nobody has run. That is a stated limitation, not
/// an oversight.
pub fn locate(runtime: Runtime) -> Option<Located> {
    if let Some(p) = std::env::var_os("APEX_AI_RUNTIME") {
        if !p.is_empty() {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Some(Located { runtime, path });
            }
            eprintln!(
                "apex-aid: APEX_AI_RUNTIME names {}, which is not a file — ignoring it",
                path.display()
            );
        }
    }
    let sysext = PathBuf::from("/usr/bin").join(runtime.program());
    if sysext.is_file() {
        return Some(Located { runtime, path: sysext });
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(runtime.program()))
        .find(|p| p.is_file())
        .map(|path| Located { runtime, path })
}

/// The first runtime this machine can actually start, or `None`.
///
/// Only runtimes `apexd_core::ai` will launch are considered, so a machine with
/// `ollama` installed does not report a runtime the daemon would then refuse.
pub fn locate_any() -> Option<Located> {
    Runtime::ALL
        .into_iter()
        .filter(|r| r.unsupported_because().is_none())
        .find_map(locate)
}


// ── resolving one load ───────────────────────────────────────────────────────

/// Everything decided about loading a model, from the store and the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The model that will be (or is) loaded.
    pub model_id: String,
    /// Its verified blob.
    pub model_path: PathBuf,
    /// Which runtime, and where it was found.
    pub located: Located,
    /// The chosen compute backend and device, with every rejection's reason.
    pub choice: BackendChoice,
    /// How much fits.
    pub fit: Fit,
    /// The argv.
    pub launch: LaunchPlan,
    /// Why the plan is what it is, in order.
    pub notes: Vec<String>,
}

/// Resolve model, runtime, backend, fit and argv.
///
/// Every decision here comes from a pure function in [`crate::ai`]; this only
/// gathers the inputs and reports the refusals.
///
/// **One implementation, two callers, and that is the point.** `apex-aid` calls
/// it to decide what to launch; `apex ai status` calls it to report what
/// *would* be launched — including on a machine where the daemon is not enabled
/// yet. Two copies would be two answers to "which backend will this machine
/// use", and the one the user was shown would not be the one that ran.
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    store: &Store,
    roots: &Roots,
    settings: &Settings,
    selected: Option<&str>,
    endpoints: &Endpoints,
    smi: &dyn NvidiaSmi,
) -> Result<Resolved, String> {
    let listed = installed(store);
    if listed.is_empty() {
        return Err(format!(
            "no models are installed in {}. Pull one with `apex ai pull <name>`, or see what \
             is available with `apex ai models --available`",
            store.root().display()
        ));
    }

    // The selected model, the configured default, or — when there is exactly
    // one — that one. Never an arbitrary choice from several: picking silently
    // would make which model answered depend on readdir order.
    let wanted = selected
        .map(str::to_string)
        .or_else(|| settings.model.clone());
    let (manifest, present) = match wanted {
        Some(id) => listed
            .into_iter()
            .find(|(m, _)| m.id == id)
            .ok_or_else(|| {
                format!("no model named {id:?} in {}", store.root().display())
            })?,
        None if listed.len() == 1 => listed.into_iter().next().expect("length checked"),
        None => {
            let names: Vec<&str> = listed.iter().map(|(m, _)| m.id.as_str()).collect();
            return Err(format!(
                "several models are installed and none is selected: {}. Choose one with \
                 `apex ai run --model <id>`, or set `model = \"<id>\"` in \
                 ~/.config/apex/ai.toml",
                names.join(", ")
            ));
        }
    };
    if !present {
        return Err(format!(
            "{} has a manifest but its weights are missing from {}. Re-pull it with \
             `sudo apex ai pull {}`",
            manifest.id,
            store.blobs_dir().display(),
            manifest.id
        ));
    }

    let runtime_kind = manifest.runtime().ok_or_else(|| {
        format!(
            "{} names runtime {:?}, which this apex does not know. It was probably pulled by \
             a newer build",
            manifest.id, manifest.runtime
        )
    })?;
    if let Some(why) = runtime_kind.unsupported_because() {
        return Err(format!("{} needs {runtime_kind}, and {why}", manifest.id));
    }

    let accel = accel(roots);
    let devices = devices(roots, smi);
    let choice = select_backend(&accel, &devices, settings.backend_pref())
        .map_err(|e| e.to_string())?;

    let located = locate(runtime_kind).ok_or_else(|| {
        format!(
            "no {runtime_kind} runtime is installed. Install one with:\n    {}",
            runtime_kind.install_hint(choice.backend)
        )
    })?;

    // Context: what the user asked for, bounded by what the weights support. A
    // request beyond the trained context is reduced rather than refused, and
    // the reduction is a note rather than a silent clamp.
    let want_context = match settings.context {
        Some(0) | None => manifest.max_context.max(MIN_USEFUL_CONTEXT),
        Some(c) => c,
    };
    let mut notes = Vec::new();
    let context = if manifest.max_context > 0 && want_context > manifest.max_context {
        notes.push(format!(
            "context reduced from {want_context} to {}, which is the largest these weights \
             were trained for",
            manifest.max_context
        ));
        manifest.max_context
    } else {
        want_context
    };

    let budget = choice
        .device
        .and_then(|i| devices.iter().find(|d| d.index == i))
        .map(Device::budget_mib)
        .unwrap_or(0);
    let fit = plan_fit(
        manifest.weights_mib,
        manifest.layers,
        manifest.kv_mib_per_1k,
        context,
        budget,
    );

    let model_path = store.blob(&manifest.digest).map_err(|e| e.to_string())?;
    let launch = plan_launch(&LaunchRequest {
        runtime: runtime_kind,
        program: &located.path.to_string_lossy(),
        model_path: &model_path,
        listen: Listen::Unix(endpoints.backend()),
        fit: &fit,
        choice: &choice,
        // 0 lets the runtime pick, which is the right default: it knows the
        // machine's core count and APEX has no better opinion for a workload
        // that is memory-bandwidth bound.
        threads: 0,
    })
    .map_err(|e| e.to_string())?;

    notes.push(choice.why.clone());
    for (b, why) in &choice.rejected {
        notes.push(format!("{b}: {why}"));
    }
    notes.extend(launch.notes.iter().cloned());

    Ok(Resolved {
        model_id: manifest.id,
        model_path,
        located,
        choice,
        fit,
        launch,
        notes,
    })
}

fn read_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::SCHEMA_VERSION;

    /// A throwaway fixture root. Named for the test so two cannot collide.
    fn fixture(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("apex-aiprobe-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&p).ok();
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn touch(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel.trim_start_matches('/'));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// An `NvidiaSmi` that reports nothing, for the fixture cases.
    struct NoSmi;
    impl NvidiaSmi for NoSmi {
        fn available(&self) -> bool {
            false
        }
        fn query(&self) -> Vec<crate::gpu::NvidiaGpu> {
            Vec::new()
        }
    }

    #[test]
    fn an_absolute_path_is_rerooted_rather_than_replacing_the_prefix() {
        // THE bug this guards: Path::join("/dev/kfd") discards the prefix, so
        // every fixture read would silently hit the real machine and the suite
        // would be testing whichever laptop ran it.
        let r = Roots { prefix: PathBuf::from("/tmp/fx") };
        assert_eq!(r.at("/dev/kfd"), PathBuf::from("/tmp/fx/dev/kfd"));
        assert_eq!(r.at("/usr/lib64/libcuda.so.1"), PathBuf::from("/tmp/fx/usr/lib64/libcuda.so.1"));
        // And an empty prefix leaves the path exactly alone.
        assert_eq!(Roots::default().at("/dev/kfd"), PathBuf::from("/dev/kfd"));
    }

    #[test]
    fn a_machine_with_only_lavapipe_reports_no_vulkan() {
        let root = fixture("lavapipe");
        touch(&root, probe_paths::VULKAN_ICD_DIR, "");
        std::fs::remove_file(root.join(probe_paths::VULKAN_ICD_DIR.trim_start_matches('/'))).ok();
        std::fs::create_dir_all(root.join(probe_paths::VULKAN_ICD_DIR.trim_start_matches('/')))
            .unwrap();
        touch(&root, &format!("{}/lvp_icd.x86_64.json", probe_paths::VULKAN_ICD_DIR), "{}");
        touch(&root, "/dev/dri/renderD128", "");

        let r = Roots { prefix: root.clone() };
        let e = accel_evidence(&r);
        assert_eq!(e.render_nodes, 1);
        assert_eq!(e.vulkan_icds, vec!["lvp_icd.x86_64.json"]);
        assert!(!e.accel().vulkan, "lavapipe was counted as a GPU");
        assert!(!e.accel().cuda);
        assert!(!e.accel().rocm);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_cuda_machine_is_recognised_from_the_two_paths_that_matter() {
        let root = fixture("cuda");
        touch(&root, probe_paths::NVIDIA_CONTROL, "");
        touch(&root, probe_paths::LIBCUDA, "");
        touch(&root, &format!("{}/nvidia_icd.x86_64.json", probe_paths::VULKAN_ICD_DIR), "{}");
        touch(&root, "/dev/dri/renderD128", "");

        let a = accel(&Roots { prefix: root.clone() });
        assert!(a.cuda);
        assert!(a.vulkan);
        assert!(!a.rocm);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_icd_listing_is_sorted_so_two_identical_machines_report_alike() {
        let root = fixture("sorted");
        for n in ["radeon_icd.x86_64.json", "intel_icd.x86_64.json", "lvp_icd.x86_64.json"] {
            touch(&root, &format!("{}/{n}", probe_paths::VULKAN_ICD_DIR), "{}");
        }
        let e = accel_evidence(&Roots { prefix: root.clone() });
        assert_eq!(
            e.vulkan_icds,
            vec!["intel_icd.x86_64.json", "lvp_icd.x86_64.json", "radeon_icd.x86_64.json"],
            "readdir order must not leak into the report"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_amdgpu_card_becomes_a_device_in_mebibytes() {
        let root = fixture("amdgpu");
        touch(&root, "/sys/class/drm/card1/device/mem_info_vram_used", "805306368\n");
        touch(&root, "/sys/class/drm/card1/device/mem_info_vram_total", "8589934592\n");
        // A connector, which carries no memory and must not appear.
        touch(&root, "/sys/class/drm/card1-eDP-1/device/mem_info_vram_used", "1\n");

        let d = devices(&Roots { prefix: root.clone() }, &NoSmi);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].total_mib, 8192);
        assert_eq!(d[0].used_mib, 768);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_intel_igpu_with_no_vram_total_is_absent_rather_than_wrong() {
        // i915/xe publish no total. A device with a made-up total would make
        // plan_fit offload a model onto memory that is not there.
        let root = fixture("i915");
        touch(&root, "/sys/class/drm/card0/device/uevent", "DRIVER=i915\n");
        assert!(devices(&Roots { prefix: root.clone() }, &NoSmi).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nvidia_devices_come_from_the_injected_querier_and_keep_their_names() {
        // The katana's real reading: RTX 3070 Laptop, 8192 MiB total, 52 used.
        struct Smi;
        impl NvidiaSmi for Smi {
            fn available(&self) -> bool {
                true
            }
            fn query(&self) -> Vec<crate::gpu::NvidiaGpu> {
                vec![crate::gpu::NvidiaGpu {
                    index: 0,
                    name: "NVIDIA GeForce RTX 3070 Laptop GPU".into(),
                    ..Default::default()
                }]
            }
            fn vram_mib(&self) -> Vec<(u32, u64, u64)> {
                vec![(0, 52, 8192)]
            }
        }
        // An empty fixture root, not Roots::default(): the default reads the
        // real /sys/class/drm, so on a machine with an AMD card this test would
        // see an extra device and the assertion would be about the developer's
        // laptop rather than about the querier.
        let root = fixture("nvidia");
        let d = devices(&Roots { prefix: root.clone() }, &Smi);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].name, "NVIDIA GeForce RTX 3070 Laptop GPU");
        assert_eq!((d[0].used_mib, d[0].total_mib), (52, 8192));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_gpu_with_memory_but_no_name_still_appears() {
        // vram_mib and query are separate driver calls; one can succeed alone.
        struct Smi;
        impl NvidiaSmi for Smi {
            fn available(&self) -> bool {
                true
            }
            fn query(&self) -> Vec<crate::gpu::NvidiaGpu> {
                Vec::new()
            }
            fn vram_mib(&self) -> Vec<(u32, u64, u64)> {
                vec![(3, 100, 4096)]
            }
        }
        let root = fixture("noname");
        let d = devices(&Roots { prefix: root.clone() }, &Smi);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].index, 3);
        assert!(d[0].name.contains('3'), "{}", d[0].name);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_desktop_with_no_mains_entry_is_not_on_battery() {
        // The failure this prevents: every desktop getting the 60-second
        // battery idle timeout because it has no power_supply directory.
        let root = fixture("desktop");
        std::fs::create_dir_all(root.join("sys/class/power_supply")).unwrap();
        assert!(!on_battery(&Roots { prefix: root.clone() }));
        // And a machine with no /sys at all.
        assert!(!on_battery(&Roots { prefix: fixture("nosys") }));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mains_offline_is_on_battery_and_mains_online_is_not() {
        for (online, expect) in [("0", true), ("1", false)] {
            let root = fixture(&format!("mains{online}"));
            touch(&root, "/sys/class/power_supply/ADP1/type", "Mains\n");
            touch(&root, "/sys/class/power_supply/ADP1/online", &format!("{online}\n"));
            touch(&root, "/sys/class/power_supply/BAT0/type", "Battery\n");
            assert_eq!(
                on_battery(&Roots { prefix: root.clone() }),
                expect,
                "online={online}"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn the_store_is_listed_with_whether_each_blob_is_actually_there() {
        let root = fixture("store");
        let store = Store::new(&root);
        std::fs::create_dir_all(store.manifests_dir()).unwrap();
        std::fs::create_dir_all(store.blobs_dir()).unwrap();

        let present = Manifest {
            version: SCHEMA_VERSION,
            id: "here".into(),
            digest: format!("sha256:{}", "1".repeat(64)),
            weights_mib: 10,
            runtime: "llama.cpp".into(),
            ..Default::default()
        };
        let missing = Manifest {
            version: SCHEMA_VERSION,
            id: "gone".into(),
            digest: format!("sha256:{}", "2".repeat(64)),
            ..Default::default()
        };
        for m in [&present, &missing] {
            std::fs::write(store.manifest(&m.id).unwrap(), m.to_json().unwrap()).unwrap();
        }
        std::fs::write(store.blob(&present.digest).unwrap(), b"weights").unwrap();
        // And a file that is not a manifest at all, which must not stop the rest.
        std::fs::write(store.manifests_dir().join("broken.json"), "{ not json").unwrap();

        let listed = installed(&store);
        assert_eq!(listed.len(), 2, "{listed:?}");
        let by_id: Vec<(String, bool)> =
            listed.into_iter().map(|(m, p)| (m.id, p)).collect();
        assert_eq!(
            by_id,
            vec![("gone".to_string(), false), ("here".to_string(), true)],
            "sorted by file name, and presence must reflect the blob"
        );

        let infos = model_infos(&store, Some("here"), Some("here"));
        let here = infos.iter().find(|i| i.id == "here").unwrap();
        assert!(here.selected && here.loaded && here.present);
        let gone = infos.iter().find(|i| i.id == "gone").unwrap();
        assert!(!gone.selected && !gone.loaded && !gone.present);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_or_absent_store_lists_nothing_rather_than_failing() {
        assert!(installed(&Store::new(Path::new("/nonexistent/apex-ai"))).is_empty());
    }

    // ── locating a runtime ───────────────────────────────────────────────────

    #[test]
    fn a_located_runtime_is_an_existing_file_the_planner_will_launch() {
        // No `set_var`: it is process-global and races other tests. What is
        // asserted instead is the invariant that has to hold on whatever
        // machine runs this.
        if let Some(l) = locate_any() {
            assert!(l.path.is_file(), "{}", l.path.display());
            assert!(l.runtime.unsupported_because().is_none(), "{}", l.runtime);
        }
        // And the search is deterministic.
        assert_eq!(
            locate(Runtime::LlamaCpp).map(|l| l.path),
            locate(Runtime::LlamaCpp).map(|l| l.path)
        );
    }

    #[test]
    fn locate_any_never_offers_a_runtime_the_planner_would_refuse() {
        // A machine with ollama installed must not report a runtime the daemon
        // then refuses to start — a status line contradicting the next command.
        let offered: Vec<Runtime> = Runtime::ALL
            .into_iter()
            .filter(|r| r.unsupported_because().is_none())
            .collect();
        assert_eq!(offered, vec![Runtime::LlamaCpp]);
    }

    // ── the shared resolver ──────────────────────────────────────────────────

    /// A store with one loadable model, and a fake runtime binary.
    fn resolvable(name: &str) -> (PathBuf, Store, Endpoints, PathBuf) {
        let root = fixture(name);
        let store = Store::new(&root.join("store"));
        std::fs::create_dir_all(store.manifests_dir()).unwrap();
        std::fs::create_dir_all(store.blobs_dir()).unwrap();
        let m = Manifest {
            version: SCHEMA_VERSION,
            id: "tiny".into(),
            digest: format!("sha256:{}", "3".repeat(64)),
            weights_mib: 512,
            layers: 16,
            kv_mib_per_1k: 8,
            max_context: 8192,
            runtime: "llama.cpp".into(),
            ..Default::default()
        };
        std::fs::write(store.manifest("tiny").unwrap(), m.to_json().unwrap()).unwrap();
        std::fs::write(store.blob(&m.digest).unwrap(), b"weights").unwrap();

        let fake = root.join("llama-server");
        std::fs::write(&fake, b"#!/bin/sh\nexit 0\n").unwrap();
        let endpoints = Endpoints::new(&root.join("run"));
        (root, store, endpoints, fake)
    }

    #[test]
    fn an_empty_store_refuses_with_the_command_that_fixes_it() {
        let root = fixture("emptystore");
        let store = Store::new(&root.join("store"));
        let e = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &Endpoints::new(&root),
            &NoSmi,
        )
        .unwrap_err();
        assert!(e.contains("apex ai pull"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_model_whose_weights_are_missing_refuses_and_says_to_re_pull() {
        let (root, store, endpoints, _fake) = resolvable("noblob");
        let m = installed(&store).remove(0).0;
        std::fs::remove_file(store.blob(&m.digest).unwrap()).unwrap();
        let e = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &endpoints,
            &NoSmi,
        )
        .unwrap_err();
        assert!(e.contains("weights are missing"), "{e}");
        assert!(e.contains("apex ai pull tiny"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn several_models_and_no_selection_refuses_rather_than_picking() {
        // Picking silently would make which model answered depend on readdir
        // order, which is the class of bug that is invisible until it matters.
        let (root, store, endpoints, _fake) = resolvable("several");
        let second = Manifest {
            version: SCHEMA_VERSION,
            id: "other".into(),
            digest: format!("sha256:{}", "4".repeat(64)),
            weights_mib: 512,
            layers: 16,
            max_context: 4096,
            runtime: "llama.cpp".into(),
            ..Default::default()
        };
        std::fs::write(store.manifest("other").unwrap(), second.to_json().unwrap()).unwrap();
        std::fs::write(store.blob(&second.digest).unwrap(), b"w").unwrap();

        let e = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &endpoints,
            &NoSmi,
        )
        .unwrap_err();
        assert!(e.contains("none is selected"), "{e}");
        assert!(e.contains("tiny") && e.contains("other"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_named_model_that_is_not_installed_refuses() {
        let (root, store, endpoints, _fake) = resolvable("named");
        let e = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            Some("absent"),
            &endpoints,
            &NoSmi,
        )
        .unwrap_err();
        assert!(e.contains("absent"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_manifest_naming_an_unknown_runtime_refuses_and_blames_version_skew() {
        // Reachable after `bootc rollback`: a model pulled by a newer apex.
        let (root, store, endpoints, _fake) = resolvable("skew");
        let mut m = installed(&store).remove(0).0;
        m.runtime = "tensorrt".into();
        std::fs::write(store.manifest("tiny").unwrap(), m.to_json().unwrap()).unwrap();
        let e = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &endpoints,
            &NoSmi,
        )
        .unwrap_err();
        assert!(e.contains("newer build"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn with_no_runtime_installed_the_refusal_carries_the_install_command() {
        // The one refusal every new machine hits first. It has to end in a
        // command, because APEX ships no runtime and never downloads one.
        let (root, store, endpoints, _fake) = resolvable("noruntime");
        match resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &endpoints,
            &NoSmi,
        ) {
            // On a machine that genuinely has llama-server on PATH the resolve
            // succeeds, and then the assertion is about the plan instead.
            Ok(r) => {
                assert_eq!(r.model_id, "tiny");
                assert!(r.launch.argv.windows(2).any(|w| w[0] == "--model"));
            }
            Err(e) => {
                assert!(e.contains("no llama.cpp runtime is installed"), "{e}");
                assert!(
                    e.contains("apex install llama-cpp") || e.contains("apex env create"),
                    "the refusal must name a command that exists: {e}"
                );
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_context_beyond_the_weights_is_reduced_with_a_note_not_refused() {
        // Only reachable when a runtime is present, so the assertion is
        // conditional — but the reduction itself is what plan_fit and the
        // resolver together must produce.
        let (root, store, endpoints, _fake) = resolvable("ctx");
        let settings = Settings { context: Some(131_072), ..Default::default() };
        if let Ok(r) = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &settings,
            None,
            &endpoints,
            &NoSmi,
        ) {
            assert!(
                r.notes.iter().any(|n| n.contains("context reduced")),
                "{:?}",
                r.notes
            );
            assert!(r.fit.context <= 8192, "{}", r.fit.context);
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_machine_with_no_accelerator_resolves_to_the_cpu_and_offloads_nothing() {
        let (root, store, endpoints, _fake) = resolvable("cpuonly");
        if let Ok(r) = resolve(
            &store,
            &Roots { prefix: root.clone() },
            &Settings::default(),
            None,
            &endpoints,
            &NoSmi,
        ) {
            assert_eq!(r.choice.backend, crate::ai::Backend::Cpu);
            assert_eq!(r.fit.placement.gpu_layers(), 0);
            assert!(
                r.launch.argv.windows(2).any(|w| w == ["--n-gpu-layers", "0"]),
                "{:?}",
                r.launch.argv
            );
            // And the backend listens on a path, never a port.
            assert!(r
                .launch
                .argv
                .windows(2)
                .any(|w| w[0] == "--host" && w[1].ends_with(".sock")));
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
