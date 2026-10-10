//! Samples physical memory footprint, RSS and CPU of an app *and every
//! process it is responsible for* (WebKit XPC helpers, extension hosts, ...)
//! while optionally opening and closing its window in a loop.
//!
//! Footprint comes from `proc_pid_rusage` (`ri_phys_footprint`), the same
//! number Activity Monitor shows in its "Memory" column. Helper processes
//! are attributed with the responsible-pid API that Activity Monitor uses
//! for its hierarchical view.

use clap::{Args, Parser, Subcommand, ValueEnum};
use paste_buff_core::paste;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CStr};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "paste-buff-bench", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sample a running or freshly launched app and write <out>.csv + <out>.json
    Run(RunArgs),
    /// Print a markdown comparison table from one or more <out>.json files
    Report { files: Vec<PathBuf> },
    /// Show which processes would be attributed to --name right now
    Ps {
        #[arg(long)]
        name: String,
    },
}

#[derive(Args)]
struct RunArgs {
    /// Label used in reports (defaults to --name or the launched binary name)
    #[arg(long)]
    label: Option<String>,
    /// Attach to running processes whose name contains this (case-insensitive)
    #[arg(long)]
    name: Option<String>,
    /// Launch this binary, measure it, and kill it afterwards
    #[arg(long)]
    launch: Option<PathBuf>,
    /// Seconds to wait after launching before sampling starts
    #[arg(long, default_value_t = 3.0)]
    warmup: f64,
    /// Seconds of idle sampling before the open/close cycles
    #[arg(long, default_value_t = 10.0)]
    idle: f64,
    /// Number of open/close cycles
    #[arg(long, default_value_t = 10)]
    cycles: u32,
    /// Seconds the window stays open in each cycle
    #[arg(long, default_value_t = 1.0)]
    hold: f64,
    /// Seconds between cycles
    #[arg(long, default_value_t = 1.0)]
    gap: f64,
    /// Seconds of idle sampling after the cycles (reveals leaks)
    #[arg(long, default_value_t = 10.0)]
    cooldown: f64,
    /// Sampling interval in seconds
    #[arg(long, default_value_t = 0.5)]
    interval: f64,
    /// How to open and close the window
    #[arg(long, value_enum, default_value_t = Toggle::Signal)]
    toggle: Toggle,
    /// Output base path; writes <out>.csv and <out>.json
    #[arg(long)]
    out: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
enum Toggle {
    /// Send SIGUSR1 (paste-buff frontends toggle their window on it)
    Signal,
    /// Post Cmd+Shift+V then Escape as HID events (needs Accessibility)
    Keys,
    /// Open Raycast's Clipboard History through its deeplink and close it by
    /// re-activating the previously frontmost app (Raycast dismisses on blur)
    Raycast,
    /// Only sample idle usage
    None,
}

const RAYCAST_CLIPBOARD_URL: &str = "raycast://extensions/raycast/clipboard-history/clipboard-history";
const RAYCAST_BUNDLE: &str = "com.raycast.macos";

// ---------------------------------------------------------------- process info

#[derive(Clone, Debug)]
struct Proc {
    pid: i32,
    ppid: i32,
    responsible: i32,
    name: String,
}

type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

/// `responsibility_get_pid_responsible_for_pid` from libquarantine (private
/// but stable; Activity Monitor relies on the same relationship).
fn load_responsible_fn() -> Option<ResponsibleFn> {
    unsafe {
        let lib = libc::dlopen(
            c"/usr/lib/system/libquarantine.dylib".as_ptr(),
            libc::RTLD_LAZY,
        );
        if lib.is_null() {
            return None;
        }
        let sym = libc::dlsym(lib, c"responsibility_get_pid_responsible_for_pid".as_ptr());
        if sym.is_null() {
            return None;
        }
        Some(std::mem::transmute::<*mut c_void, ResponsibleFn>(sym))
    }
}

fn all_pids() -> Vec<i32> {
    unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        let mut buf = vec![0i32; (count.max(0) as usize) * 2 + 64];
        let bytes = (buf.len() * std::mem::size_of::<i32>()) as i32;
        let got = libc::proc_listallpids(buf.as_mut_ptr() as *mut c_void, bytes);
        buf.truncate(got.max(0) as usize);
        buf.retain(|&p| p > 0);
        buf
    }
}

fn proc_path(pid: i32) -> Option<String> {
    let mut buf = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    (got == size).then_some(info)
}

fn list_procs(responsible: Option<ResponsibleFn>) -> Vec<Proc> {
    all_pids()
        .into_iter()
        .filter_map(|pid| {
            let info = bsd_info(pid)?;
            let name = proc_path(pid)
                .and_then(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| {
                    unsafe { CStr::from_ptr(info.pbi_name.as_ptr()) }
                        .to_string_lossy()
                        .into_owned()
                });
            let resp = responsible.map(|f| unsafe { f(pid) }).unwrap_or(pid);
            Some(Proc {
                pid,
                ppid: info.pbi_ppid as i32,
                responsible: resp,
                name,
            })
        })
        .collect()
}

/// Everything that belongs to the target: name matches and `roots`, plus any
/// process whose responsible pid or parent is already in the set (repeated
/// until nothing changes).
fn attribute<'a>(procs: &'a [Proc], roots: &HashSet<i32>, name: Option<&str>) -> Vec<&'a Proc> {
    let mut set = roots.clone();
    if let Some(name) = name {
        let needle = name.to_lowercase();
        for p in procs {
            if p.name.to_lowercase().contains(&needle) {
                set.insert(p.pid);
            }
        }
    }
    loop {
        let before = set.len();
        for p in procs {
            if !set.contains(&p.pid)
                && (set.contains(&p.responsible) || (p.ppid > 1 && set.contains(&p.ppid)))
            {
                set.insert(p.pid);
            }
        }
        if set.len() == before {
            break;
        }
    }
    let me = std::process::id() as i32;
    procs
        .iter()
        .filter(|p| set.contains(&p.pid) && p.pid != me)
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
struct Usage {
    footprint: u64,
    rss: u64,
    cpu_ns: u64,
}

#[allow(deprecated)]
fn timebase() -> (u64, u64) {
    let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
    unsafe { libc::mach_timebase_info(&mut tb) };
    (tb.numer.max(1) as u64, tb.denom.max(1) as u64)
}

fn usage(pid: i32, tb: (u64, u64)) -> Option<Usage> {
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // The C signature takes `rusage_info_t *` where rusage_info_t is `void *`,
    // but the kernel expects a pointer to the struct itself.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            &mut info as *mut libc::rusage_info_v4 as *mut libc::rusage_info_t,
        )
    };
    if rc != 0 {
        return None;
    }
    let ticks = info.ri_user_time + info.ri_system_time;
    Some(Usage {
        footprint: info.ri_phys_footprint,
        rss: info.ri_resident_size,
        cpu_ns: ticks * tb.0 / tb.1,
    })
}

// ------------------------------------------------------------------ sampling

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Sample {
    t_s: f64,
    phase: String,
    procs: usize,
    footprint_bytes: u64,
    rss_bytes: u64,
    cpu_pct: f64,
}

struct Sampler {
    responsible: Option<ResponsibleFn>,
    tb: (u64, u64),
    roots: HashSet<i32>,
    name: Option<String>,
    /// Pids that existed before we launched the target (launch mode only).
    /// Any *new* WebKit XPC process is attributed to the target, because an
    /// app launched from a terminal is not its own responsible process.
    preexisting: Option<HashSet<i32>>,
    start: Instant,
    last_cpu: HashMap<i32, (u64, Instant)>,
    samples: Vec<Sample>,
    last_procs: Vec<Proc>,
}

impl Sampler {
    fn new(roots: HashSet<i32>, name: Option<String>, preexisting: Option<HashSet<i32>>) -> Self {
        Self {
            responsible: load_responsible_fn(),
            tb: timebase(),
            roots,
            name,
            preexisting,
            start: Instant::now(),
            last_cpu: HashMap::new(),
            samples: Vec::new(),
            last_procs: Vec::new(),
        }
    }

    fn targets(&self) -> Vec<Proc> {
        let procs = list_procs(self.responsible);
        let mut roots = self.roots.clone();
        if let Some(pre) = &self.preexisting {
            for p in &procs {
                if !pre.contains(&p.pid) && p.name.to_lowercase().contains("webkit") {
                    roots.insert(p.pid);
                }
            }
        }
        attribute(&procs, &roots, self.name.as_deref())
            .into_iter()
            .cloned()
            .collect()
    }

    fn sample(&mut self, phase: &str) -> Sample {
        let now = Instant::now();
        let targets = self.targets();
        let mut footprint = 0;
        let mut rss = 0;
        let mut cpu_pct = 0.0;
        let mut seen = HashMap::new();
        for p in &targets {
            if let Some(u) = usage(p.pid, self.tb) {
                footprint += u.footprint;
                rss += u.rss;
                if let Some((prev_cpu, prev_t)) = self.last_cpu.get(&p.pid) {
                    let wall = now.duration_since(*prev_t).as_nanos() as f64;
                    if wall > 0.0 {
                        cpu_pct += (u.cpu_ns.saturating_sub(*prev_cpu)) as f64 / wall * 100.0;
                    }
                }
                seen.insert(p.pid, (u.cpu_ns, now));
            }
        }
        self.last_cpu = seen;
        self.last_procs = targets;
        let s = Sample {
            t_s: now.duration_since(self.start).as_secs_f64(),
            phase: phase.to_string(),
            procs: self.last_procs.len(),
            footprint_bytes: footprint,
            rss_bytes: rss,
            cpu_pct,
        };
        self.samples.push(s.clone());
        s
    }

    /// Sample every `interval` for `duration`.
    fn sample_for(&mut self, duration: Duration, interval: Duration, phase: &str) {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            let s = self.sample(phase);
            eprint!(
                "\r  [{phase:>8}] t={:6.1}s procs={} footprint={} cpu={:.1}%   ",
                s.t_s,
                s.procs,
                mb(s.footprint_bytes),
                s.cpu_pct
            );
            thread::sleep(interval);
        }
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn secs(s: f64) -> Duration {
    Duration::from_secs_f64(s.max(0.0))
}

// -------------------------------------------------------------------- toggling

struct Toggler {
    mode: Toggle,
    root: Option<i32>,
    /// Bundle id of the app that was frontmost before Raycast opened.
    prev_bundle: Option<String>,
}

impl Toggler {
    fn open(&mut self) {
        match self.mode {
            Toggle::Signal => {
                if let Some(pid) = self.root {
                    unsafe { libc::kill(pid, libc::SIGUSR1) };
                }
            }
            Toggle::Keys => {
                if let Err(e) = paste::send_cmd_shift_v() {
                    eprintln!("\n  could not post Cmd+Shift+V: {e}");
                }
            }
            Toggle::Raycast => {
                self.prev_bundle = paste::frontmost_app()
                    .and_then(|a| a.bundle_id)
                    .filter(|b| b != RAYCAST_BUNDLE);
                let _ = Command::new("open")
                    .arg(RAYCAST_CLIPBOARD_URL)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            Toggle::None => {}
        }
    }

    fn close(&mut self) {
        match self.mode {
            Toggle::Signal => self.open(),
            Toggle::Keys => {
                if let Err(e) = paste::send_escape() {
                    eprintln!("\n  could not post Escape: {e}");
                }
            }
            Toggle::Raycast => {
                let bundle = self
                    .prev_bundle
                    .clone()
                    .unwrap_or_else(|| "com.apple.finder".to_string());
                let _ = Command::new("open")
                    .args(["-b", &bundle])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            Toggle::None => {}
        }
    }
}

// --------------------------------------------------------------------- summary

#[derive(Serialize, Deserialize, Debug)]
struct Summary {
    label: String,
    target: String,
    toggle: String,
    cycles: u32,
    samples: usize,
    procs_max: usize,
    procs_final: usize,
    proc_names: Vec<String>,
    idle_footprint_avg: u64,
    idle_footprint_max: u64,
    cycling_footprint_avg: u64,
    cycling_footprint_max: u64,
    after_footprint_avg: u64,
    final_footprint: u64,
    idle_rss_avg: u64,
    peak_rss: u64,
    idle_cpu_avg: f64,
    cycling_cpu_avg: f64,
    after_cpu_avg: f64,
    binary_bytes: Option<u64>,
    recorded_at_unix: u64,
}

fn avg_u64(samples: &[Sample], phase: &str, f: impl Fn(&Sample) -> u64) -> u64 {
    let v: Vec<u64> = samples.iter().filter(|s| s.phase == phase).map(&f).collect();
    if v.is_empty() {
        0
    } else {
        v.iter().sum::<u64>() / v.len() as u64
    }
}

fn max_u64(samples: &[Sample], phase: &str, f: impl Fn(&Sample) -> u64) -> u64 {
    samples.iter().filter(|s| s.phase == phase).map(&f).max().unwrap_or(0)
}

fn avg_f64(samples: &[Sample], phase: &str, f: impl Fn(&Sample) -> f64) -> f64 {
    // Skip the first sample of each phase: its CPU delta spans the previous phase.
    let v: Vec<f64> = samples
        .iter()
        .filter(|s| s.phase == phase)
        .skip(1)
        .map(&f)
        .collect();
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

fn summarize(label: String, target: String, args: &RunArgs, sampler: &Sampler, binary: Option<&Path>) -> Summary {
    let s = &sampler.samples;
    let mut names: Vec<String> = sampler.last_procs.iter().map(|p| p.name.clone()).collect();
    names.sort();
    names.dedup();
    Summary {
        label,
        target,
        toggle: format!("{:?}", args.toggle).to_lowercase(),
        cycles: args.cycles,
        samples: s.len(),
        procs_max: s.iter().map(|x| x.procs).max().unwrap_or(0),
        procs_final: sampler.last_procs.len(),
        proc_names: names,
        idle_footprint_avg: avg_u64(s, "idle", |x| x.footprint_bytes),
        idle_footprint_max: max_u64(s, "idle", |x| x.footprint_bytes),
        cycling_footprint_avg: avg_u64(s, "cycling", |x| x.footprint_bytes),
        cycling_footprint_max: max_u64(s, "cycling", |x| x.footprint_bytes),
        after_footprint_avg: avg_u64(s, "after", |x| x.footprint_bytes),
        final_footprint: s.last().map(|x| x.footprint_bytes).unwrap_or(0),
        idle_rss_avg: avg_u64(s, "idle", |x| x.rss_bytes),
        peak_rss: s.iter().map(|x| x.rss_bytes).max().unwrap_or(0),
        idle_cpu_avg: avg_f64(s, "idle", |x| x.cpu_pct),
        cycling_cpu_avg: avg_f64(s, "cycling", |x| x.cpu_pct),
        after_cpu_avg: avg_f64(s, "after", |x| x.cpu_pct),
        binary_bytes: binary.and_then(|p| fs::metadata(p).ok()).map(|m| m.len()),
        recorded_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

fn write_outputs(out: &Path, samples: &[Sample], summary: &Summary) -> std::io::Result<()> {
    if let Some(dir) = out.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    let csv_path = out.with_extension("csv");
    let mut csv = fs::File::create(&csv_path)?;
    writeln!(csv, "t_s,phase,procs,footprint_bytes,rss_bytes,cpu_pct")?;
    for s in samples {
        writeln!(
            csv,
            "{:.2},{},{},{},{},{:.2}",
            s.t_s, s.phase, s.procs, s.footprint_bytes, s.rss_bytes, s.cpu_pct
        )?;
    }
    let json_path = out.with_extension("json");
    fs::write(&json_path, serde_json::to_vec_pretty(summary)?)?;
    eprintln!("\nwrote {} and {}", csv_path.display(), json_path.display());
    Ok(())
}

// ------------------------------------------------------------------------ run

fn run(args: RunArgs) -> Result<(), String> {
    if args.name.is_none() && args.launch.is_none() {
        return Err("pass --name <substring> and/or --launch <binary>".into());
    }
    if args.toggle == Toggle::Keys && !paste::accessibility_trusted() {
        return Err(
            "--toggle keys needs the Accessibility permission for this terminal \
             (System Settings → Privacy & Security → Accessibility). \
             Use --toggle signal for paste-buff frontends or --toggle none."
                .into(),
        );
    }

    let mut child: Option<Child> = None;
    let mut roots = HashSet::new();
    let mut preexisting = None;
    if let Some(bin) = &args.launch {
        preexisting = Some(all_pids().into_iter().collect::<HashSet<i32>>());
        let log = fs::File::create(args.out.with_extension("log")).map_err(|e| e.to_string())?;
        let err_log = log.try_clone().map_err(|e| e.to_string())?;
        let c = Command::new(bin)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log))
            .spawn()
            .map_err(|e| format!("launch {}: {e}", bin.display()))?;
        eprintln!("launched {} as pid {}", bin.display(), c.id());
        roots.insert(c.id() as i32);
        child = Some(c);
        thread::sleep(secs(args.warmup));
    }

    let label = args
        .label
        .clone()
        .or_else(|| args.name.clone())
        .or_else(|| {
            args.launch
                .as_ref()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        })
        .unwrap_or_else(|| "target".into());
    let target_desc = args
        .launch
        .as_ref()
        .map(|p| p.display().to_string())
        .or_else(|| args.name.clone())
        .unwrap_or_default();

    let mut sampler = Sampler::new(roots.clone(), args.name.clone(), preexisting);
    let initial = sampler.targets();
    if initial.is_empty() {
        if let Some(mut c) = child {
            let _ = c.kill();
        }
        return Err(format!("no processes matched {target_desc:?}"));
    }
    eprintln!("tracking {} process(es):", initial.len());
    for p in &initial {
        eprintln!("  {:>6}  {}", p.pid, p.name);
    }
    let root = roots.iter().next().copied().or_else(|| initial.first().map(|p| p.pid));
    let interval = secs(args.interval);
    let mut toggler = Toggler {
        mode: args.toggle,
        root,
        prev_bundle: None,
    };

    eprintln!("idle phase ({}s)", args.idle);
    sampler.sample_for(secs(args.idle), interval, "idle");

    if args.toggle != Toggle::None && args.cycles > 0 {
        eprintln!("\ncycling {} × (open {}s, closed {}s)", args.cycles, args.hold, args.gap);
        for _ in 0..args.cycles {
            toggler.open();
            sampler.sample_for(secs(args.hold), interval, "cycling");
            toggler.close();
            sampler.sample_for(secs(args.gap), interval, "cycling");
        }
        eprintln!("\nafter phase ({}s)", args.cooldown);
        sampler.sample_for(secs(args.cooldown), interval, "after");
    }

    let summary = summarize(label, target_desc, &args, &sampler, args.launch.as_deref());
    write_outputs(&args.out, &sampler.samples, &summary).map_err(|e| e.to_string())?;
    print_table(&[summary]);

    if let Some(mut c) = child {
        unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
        thread::sleep(Duration::from_millis(300));
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(())
}

fn print_table(rows: &[Summary]) {
    println!(
        "| target | procs | idle footprint | peak (cycling) | after cycles | idle RSS | idle CPU | cycling CPU | binary |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
    for s in rows {
        println!(
            "| {} | {} | {} | {} | {} | {} | {:.1}% | {:.1}% | {} |",
            s.label,
            s.procs_max,
            mb(s.idle_footprint_avg),
            mb(s.cycling_footprint_max),
            if s.after_footprint_avg > 0 { mb(s.after_footprint_avg) } else { "–".into() },
            mb(s.idle_rss_avg),
            s.idle_cpu_avg,
            s.cycling_cpu_avg,
            s.binary_bytes.map(mb).unwrap_or_else(|| "–".into()),
        );
    }
}

fn report(files: Vec<PathBuf>) -> Result<(), String> {
    let mut rows = Vec::new();
    for f in files {
        let bytes = fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let s: Summary = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", f.display()))?;
        rows.push(s);
    }
    rows.sort_by_key(|s| s.idle_footprint_avg);
    print_table(&rows);
    println!();
    for s in &rows {
        println!("- **{}**: {} process(es) — {}", s.label, s.procs_max, s.proc_names.join(", "));
    }
    Ok(())
}

fn ps(name: String) -> Result<(), String> {
    let sampler = Sampler::new(HashSet::new(), Some(name.clone()), None);
    let targets = sampler.targets();
    if targets.is_empty() {
        return Err(format!("no processes match {name:?}"));
    }
    let mut total = 0;
    println!("{:>7} {:>7} {:>7} {:>12}  name", "pid", "ppid", "resp", "footprint");
    for p in &targets {
        let u = usage(p.pid, sampler.tb).unwrap_or_default();
        total += u.footprint;
        println!(
            "{:>7} {:>7} {:>7} {:>12}  {}",
            p.pid,
            p.ppid,
            p.responsible,
            mb(u.footprint),
            p.name
        );
    }
    println!("{:>36}  total", mb(total));
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Run(args) => run(args),
        Cmd::Report { files } => report(files),
        Cmd::Ps { name } => ps(name),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
