//! Event server: spawn, poll, shutdown, and cleanup for the HTTP event server.
//!
//! The event server is a Bun subprocess that workers POST events to.
//! The orchestrator's stop hook long-polls instead of spinning, reducing
//! token cost from ~3600 calls/hour to ~120/hour.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::file::{get_ppid, sessions_dir};

/// Embedded event server TypeScript source (zero-dependency Bun HTTP server).
const EVENT_SERVER_SCRIPT: &str = r#"// Bacchus event server — zero-dependency Bun HTTP server
// Spawned by `bacchus session start orchestrator`, self-terminates when owner PID dies.

const OWNER_PID = parseInt(Bun.env.BACCHUS_OWNER_PID || "0", 10);
const RUN_ID = Bun.env.BACCHUS_RUN_ID || "unknown";
const PROJECT_DIR = Bun.env.CLAUDE_PROJECT_DIR || ".";
const MAX_EVENTS = 200;

type Event = {
  type: string;
  task_id?: string;
  agent_id?: string;
  activity?: string;
  ts: number;
};

type PollResult = {
  events: Event[];
  elapsed_ms: number;
  shutdown?: boolean;
};

type Waiter = {
  resolve: (r: PollResult) => void;
  timer: ReturnType<typeof setTimeout>;
};

const events: Event[] = [];
const waiters: Waiter[] = [];
const startTime = Date.now();

function enqueueEvent(event: Event) {
  events.push(event);
  if (events.length > MAX_EVENTS) events.shift();
  for (const w of waiters.splice(0)) {
    clearTimeout(w.timer);
    w.resolve({ events: [event], elapsed_ms: 0 });
  }
}

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

async function handleRequest(req: Request): Promise<Response> {
  const url = new URL(req.url);

  if (req.method === "POST" && url.pathname === "/event") {
    const body: Record<string, unknown> = await req.json().catch(() => ({}));
    const event: Event = {
      type: (body.type as string) || "unknown",
      task_id: body.task_id as string | undefined,
      agent_id: body.agent_id as string | undefined,
      activity: body.activity as string | undefined,
      ts: Date.now(),
    };
    enqueueEvent(event);
    return json({ ok: true });
  }

  if (req.method === "POST" && url.pathname === "/heartbeat") {
    const body: Record<string, unknown> = await req.json().catch(() => ({}));
    const task_id = body.task_id as string | undefined;
    const agent_id = body.agent_id as string | undefined;
    const activity = (body.activity as string) || "working";

    if (task_id && agent_id) {
      // Update SQLite heartbeat via CLI (fire-and-forget)
      Bun.spawn(["bacchus", "activity", task_id, agent_id, activity], {
        stdout: "ignore",
        stderr: "ignore",
      });
    }

    // Also queue as event for orchestrator long-poll wake
    enqueueEvent({ type: "heartbeat", task_id, agent_id, activity, ts: Date.now() });
    return json({ ok: true });
  }

  if (req.method === "GET" && url.pathname === "/poll") {
    const timeout = Math.min(
      parseInt(url.searchParams.get("timeout") || "28000", 10),
      60000,
    );
    const since = parseInt(url.searchParams.get("since") || "0", 10);

    // Return buffered events if any
    const recent = since > 0 ? events.filter((e) => e.ts > since) : [];
    if (recent.length > 0) {
      return json({ events: recent, elapsed_ms: 0 });
    }

    // Long-poll: block until event arrives or timeout
    const start = Date.now();
    const result = await new Promise<PollResult>((resolve) => {
      const timer = setTimeout(() => {
        const idx = waiters.findIndex((w) => w.resolve === resolve);
        if (idx !== -1) waiters.splice(idx, 1);
        resolve({ events: [], elapsed_ms: Date.now() - start });
      }, timeout);
      waiters.push({ resolve, timer });
    });
    return json(result);
  }

  if (req.method === "GET" && url.pathname === "/status") {
    return json({
      run_id: RUN_ID,
      uptime_ms: Date.now() - startTime,
      queue_len: events.length,
      waiters: waiters.length,
    });
  }

  if (req.method === "POST" && url.pathname === "/shutdown") {
    for (const w of waiters.splice(0)) {
      clearTimeout(w.timer);
      w.resolve({ events: [], elapsed_ms: 0, shutdown: true });
    }
    setTimeout(() => process.exit(0), 100);
    return json({ ok: true, shutting_down: true });
  }

  return json({ error: "not found" }, 404);
}

// Owner PID watchdog — exit when parent dies
if (OWNER_PID > 0) {
  setInterval(() => {
    try {
      process.kill(OWNER_PID, 0);
    } catch {
      process.exit(0);
    }
  }, 15000);
}

const server = Bun.serve({
  fetch: handleRequest,
  port: 0,
  hostname: "127.0.0.1",
});

// Write port file so orchestrator can find us
const portFile = `${PROJECT_DIR}/.bacchus/sessions/server_port_${RUN_ID}`;
await Bun.write(portFile, String(server.port));

console.error(
  `bacchus-event-server: port=${server.port} run_id=${RUN_ID} pid=${process.pid}`,
);
"#;

/// Path to the port file for a given run_id.
pub(super) fn server_port_path(run_id: &str) -> PathBuf {
    sessions_dir()
        .unwrap_or_else(|| PathBuf::from(".bacchus/sessions"))
        .join(format!("server_port_{}", run_id))
}

/// Read the event server port from the port file.
pub(super) fn read_server_port(run_id: &str) -> Option<u16> {
    let path = server_port_path(run_id);
    std::fs::read_to_string(&path)
        .ok()?
        .trim()
        .parse::<u16>()
        .ok()
}

/// Write the event server script to `.bacchus/server.ts` (idempotent overwrite).
fn ensure_event_server_script(workspace_root: &Path) -> Result<(), String> {
    let path = workspace_root.join(".bacchus/server.ts");
    std::fs::write(&path, EVENT_SERVER_SCRIPT).map_err(|e| e.to_string())
}

/// Spawn the event server as a detached Bun subprocess.
///
/// Writes the server script if missing, then spawns bun.
/// Waits up to 3s for the port file to appear and validate.
pub(super) fn spawn_event_server(workspace_root: &Path, run_id: &str) -> Result<(), String> {
    // Check bun is available
    let bun_ok = Command::new("bun")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !bun_ok {
        return Err("bun not available".to_string());
    }

    ensure_event_server_script(workspace_root)?;

    let script_path = workspace_root.join(".bacchus/server.ts");
    let sessions_dir = workspace_root.join(".bacchus/sessions");
    std::fs::create_dir_all(&sessions_dir).map_err(|e| e.to_string())?;

    // Use parent PID (the Claude Code process) so the server exits when Claude dies.
    let owner_pid = get_ppid().unwrap_or(std::process::id());

    let mut cmd = Command::new("bun");
    cmd.arg("run")
        .arg(&script_path)
        .env("BACCHUS_RUN_ID", run_id)
        .env("BACCHUS_OWNER_PID", owner_pid.to_string())
        .env(
            "CLAUDE_PROJECT_DIR",
            workspace_root.to_string_lossy().as_ref(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    cmd.spawn()
        .map_err(|e| format!("failed to spawn event server: {}", e))?;

    // Wait up to 3s for port file to appear with valid content (avoids TOCTOU)
    for _ in 0..30 {
        if read_server_port(run_id).is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    Err("event server did not write port file within 3s".to_string())
}

/// Long-poll the event server. Returns raw JSON response or None on failure.
pub(super) fn event_server_poll(port: u16, timeout_ms: u32) -> Option<String> {
    let timeout_secs = (timeout_ms / 1000) + 2;
    let url = format!("http://127.0.0.1:{}/poll?timeout={}", port, timeout_ms);

    let response = ureq::get(&url)
        .timeout(Duration::from_secs(timeout_secs.into()))
        .call()
        .ok()?;

    let body = response.into_string().ok()?;
    if body.trim().is_empty() {
        return None;
    }
    Some(body)
}

/// Send a shutdown request to the event server (best-effort).
pub(super) fn shutdown_event_server(port: u16) {
    let url = format!("http://127.0.0.1:{}/shutdown", port);
    let _ = ureq::post(&url).timeout(Duration::from_secs(2)).call();
}

/// Delete the server port file.
pub(super) fn cleanup_server_port_file(run_id: &str) {
    let path = server_port_path(run_id);
    let _ = std::fs::remove_file(&path);
}

/// Remove orphaned port files not referenced by any active session.
pub(super) fn prune_orphaned_port_files(active_run_ids: &std::collections::HashSet<String>) {
    let dir = match sessions_dir() {
        Some(d) if d.exists() => d,
        _ => return,
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if let Some(run_id) = name.strip_prefix("server_port_") {
            if !active_run_ids.contains(run_id) {
                // Try to shut down the server before removing the port file
                if let Some(port) = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|s| s.trim().parse::<u16>().ok())
                {
                    shutdown_event_server(port);
                }
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}
