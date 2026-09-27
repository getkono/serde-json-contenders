//! End to end: a hyper server per backend, counted in-process and timed over
//! loopback.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::build::{Place, bin, build};
use crate::counted::{CACHE, Filter, estimated_cycles, parse_callgrind};
use crate::util::{capture, parallel, root, shuffle};

/// Routes, with the method, path and request body each is driven with.
pub const ROUTES: &[(&str, &str, &str, Option<&str>)] = &[
    ("echo-post", "POST", "/echo", Some("echo-post")),
    ("json-large-get", "GET", "/json/large", None),
    ("json-large-post", "POST", "/json/large", Some("json-large")),
    ("json-small-get", "GET", "/json/small", None),
];

/// Backends a set serves end to end; the baseline also serves the floor.
fn backends(set: &crate::matrix::Set) -> Vec<&'static str> {
    let mut b: Vec<&'static str> = set.backends.to_vec();
    if set.name == "baseline" {
        b.push("floor");
    }
    b
}

/// Routes a backend can serve: decode-only backends only the decode route.
fn routes_for(backend: &str) -> Vec<&'static (&'static str, &'static str, &'static str, Option<&'static str>)> {
    let decode_only = matches!(backend, "jiter" | "hifijson");
    ROUTES
        .iter()
        .filter(|r| !decode_only || r.0 == "json-large-post")
        .collect()
}

/// `xtask e2e-count`: callgrind instructions per request, in the container.
pub fn count(args: &[String]) -> Result<()> {
    if std::env::var_os("SJC_IN_CONTAINER").is_none() {
        return crate::container::reexec("e2e-count", args);
    }
    let filter = Filter::parse(args);
    let requests = 200;
    let mut rows = Vec::new();
    for variant in filter.variants().into_iter().filter(|v| v.valgrind) {
        for set in filter.sets() {
            let release = build(Place::Container, "e2e", set, &variant)?;
            let exe = bin(&release, "e2e-count");
            let scratch = root().join("target/callgrind-e2e").join(variant.name).join(set.name);
            std::fs::create_dir_all(&scratch)?;
            let mut jobs: Vec<Box<dyn FnOnce() -> Value + Send>> = Vec::new();
            for backend in backends(set) {
                for route in routes_for(backend) {
                    let exe = exe.clone();
                    let out = scratch.join(format!("{backend}-{}.out", route.0));
                    let (variant, set_name) = (variant.name, set.name);
                    jobs.push(Box::new(move || {
                        let label = crate::matrix::label(set, backend);
                        let mut row = json!({ "variant": variant, "set": set_name, "backend": label, "route": route.0, "requests": requests });
                        let mut cmd = Command::new("valgrind");
                        cmd.args(["--tool=callgrind", "--collect-atstart=no", "--toggle-collect=*e2e_measured*", "--cache-sim=yes"])
                            .args(CACHE)
                            .arg(format!("--callgrind-out-file={}", out.display()))
                            .arg(&exe)
                            .args([backend, route.0, &requests.to_string()]);
                        match capture(&mut cmd) {
                            Ok((true, _, _)) => match std::fs::read_to_string(&out).map_err(anyhow::Error::from).and_then(|t| parse_callgrind(&t)) {
                                Ok(e) => {
                                    row["ir"] = json!(e.ir as f64 / f64::from(requests));
                                    row["est_cycles"] = json!(estimated_cycles(&e) as f64 / f64::from(requests));
                                }
                                Err(e) => row["error"] = json!(e.to_string()),
                            },
                            Ok((false, _, stderr)) => row["error"] = json!(stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n")),
                            Err(e) => row["error"] = json!(e.to_string()),
                        }
                        let _ = std::fs::remove_file(&out);
                        row
                    }));
                }
            }
            rows.extend(parallel(8, jobs));
        }
    }
    let path = root()
        .join("results/callgrind")
        .join(format!("e2e-{}.json", std::env::consts::ARCH));
    crate::util::merge_write(
        &path,
        "e2e-count",
        &rows,
        &["variant", "set", "backend", "route"],
        json!({}),
    )
}

/// The backend name a server takes, from a row label: the A/A copy and the
/// float-roundtrip build are serde_json.
fn server_name(label: &str) -> &str {
    label.trim_end_matches("#aa").trim_end_matches("+float_roundtrip")
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn taskset(cores: &str, program: &std::path::Path) -> Command {
    if cfg!(target_os = "linux") {
        let mut cmd = Command::new("taskset");
        cmd.args(["-c", cores]).arg(program);
        cmd
    } else {
        Command::new(program)
    }
}

fn start(exe: &std::path::Path, backend: &str, port: u16) -> Result<Server> {
    let mut child = taskset("0-3", exe)
        .args([backend, &port.to_string(), "4"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("starting e2e-server")?;
    let stdout = child.stdout.take().context("server stdout")?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    anyhow::ensure!(line.starts_with("ready"), "server said {line:?}");
    Ok(Server(child))
}

fn oha(route: &(&str, &str, &str, Option<&str>), port: u16, seconds: u64, rate: Option<f64>) -> Result<Value> {
    let program = which("oha")?;
    let mut cmd = taskset("8-15", &program);
    cmd.args([
        "--no-tui",
        "--output-format",
        "json",
        "-z",
        &format!("{seconds}s"),
        "-c",
        "64",
        "-m",
        route.1,
    ]);
    if let Some(workload) = route.3 {
        let body = root().join("target/e2e-bodies").join(format!("{workload}.json"));
        cmd.args(["-D"]).arg(body).args(["-T", "application/json"]);
    }
    if let Some(rate) = rate {
        cmd.args(["--latency-correction", "-q", &format!("{}", rate.round() as u64)]);
    }
    cmd.arg(format!("http://127.0.0.1:{port}{}", route.2));
    let (ok, stdout, stderr) = capture(&mut cmd)?;
    anyhow::ensure!(ok, "oha failed: {stderr}");
    let v: Value = serde_json::from_str(&stdout)?;
    let codes = v["statusCodeDistribution"].clone();
    Ok(json!({
        "rps": v["summary"]["requestsPerSec"],
        "success_rate": v["summary"]["successRate"],
        "p50_ms": v["latencyPercentiles"]["p50"].as_f64().map(|s| s * 1e3),
        "p99_ms": v["latencyPercentiles"]["p99"].as_f64().map(|s| s * 1e3),
        "p999_ms": v["latencyPercentiles"]["p99.9"].as_f64().map(|s| s * 1e3),
        "status_codes": codes,
    }))
}

fn which(name: &str) -> Result<std::path::PathBuf> {
    let (ok, out, _) = capture(Command::new("mise").args(["which", name]))?;
    if ok {
        return Ok(out.trim().into());
    }
    Ok(name.into())
}

/// `xtask e2e-time`: closed-loop throughput, then open-loop latency at 70 % of
/// serde_json's throughput on the same route, `--reps` times (default 5),
/// with serde_json run twice per rep as the A/A control.
pub fn time(args: &[String]) -> Result<()> {
    let mut filter = Filter::parse(args);
    if filter.variants.is_none() {
        filter.variants = Some(vec!["native".into()]);
    }
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let reps = flag("--reps").unwrap_or(5);
    let seconds = flag("--seconds").unwrap_or(30);
    let trust = if args.iter().any(|a| a == "--canonical") {
        "canonical"
    } else {
        "solo"
    };
    let bodies = root().join("target/e2e-bodies");
    std::fs::create_dir_all(&bodies)?;
    std::fs::write(
        bodies.join("echo-post.json"),
        <payloads::kynos::EchoPost as payloads::Workload>::bytes(),
    )?;
    std::fs::write(
        bodies.join("json-large.json"),
        <payloads::kynos::JsonLarge as payloads::Workload>::bytes(),
    )?;
    let mut rows = Vec::new();
    for variant in filter.variants() {
        let mut servers = Vec::new();
        for set in filter.sets() {
            let exe = bin(&build(Place::Host, "e2e", set, &variant)?, "e2e-server");
            for backend in set.backends {
                servers.push((set.name, crate::matrix::label(set, backend), exe.clone()));
                if set.name == "baseline" {
                    servers.push((set.name, "serde_json#aa".to_owned(), exe.clone()));
                }
            }
        }
        for route in ROUTES {
            let eligible: Vec<_> = servers
                .iter()
                .filter(|(_, b, _)| routes_for(server_name(b)).iter().any(|r| r.0 == route.0))
                .collect();
            let mut results: Vec<Vec<Value>> = vec![Vec::new(); eligible.len()];
            let mut rate = None;
            for rep in 0..reps {
                let mut order: Vec<usize> = (0..eligible.len()).collect();
                shuffle(&mut order, 0xe2e + rep);
                // The open-loop rate is fixed from serde_json's first closed-loop run.
                if rate.is_none()
                    && let Some(i) = eligible.iter().position(|(_, b, _)| b == "serde_json")
                {
                    order.retain(|&j| j != i);
                    order.insert(0, i);
                }
                for &i in &order {
                    let (_, backend, exe) = eligible[i];
                    eprintln!("e2e-time: {} rep {}/{reps} {}", route.0, rep + 1, backend);
                    let port = 18_000 + u16::try_from(i).unwrap_or(0);
                    let server = start(exe, server_name(backend), port)?;
                    let closed = oha(route, port, seconds, None)?;
                    if rate.is_none() && backend == "serde_json" {
                        rate = closed["rps"].as_f64().map(|r| r * 0.7);
                    }
                    let open = match rate {
                        Some(r) => oha(route, port, seconds, Some(r))?,
                        None => Value::Null,
                    };
                    drop(server);
                    results[i].push(json!({ "closed": closed, "open": open }));
                }
            }
            for ((set, backend, _), reps) in eligible.iter().zip(results) {
                let mut rps: Vec<f64> = reps.iter().filter_map(|r| r["closed"]["rps"].as_f64()).collect();
                let mut p99: Vec<f64> = reps.iter().filter_map(|r| r["open"]["p99_ms"].as_f64()).collect();
                rps.sort_by(f64::total_cmp);
                p99.sort_by(f64::total_cmp);
                rows.push(json!({
                    "variant": variant.name, "set": set, "backend": backend, "route": route.0, "trust": trust, "seconds": seconds,
                    "rps": rps.get(rps.len() / 2), "rps_spread": rps.last().zip(rps.first()).map(|(a, b)| a - b),
                    "open_loop_rate": rate, "p99_ms": p99.get(p99.len() / 2), "p99_spread": p99.last().zip(p99.first()).map(|(a, b)| a - b),
                    "reps": reps,
                }));
            }
        }
    }
    let path = root()
        .join("results")
        .join(crate::provenance::host_slug())
        .join("e2e-time.json");
    crate::util::merge_write(
        &path,
        "e2e-time",
        &rows,
        &["variant", "set", "backend", "route"],
        json!({}),
    )
}
