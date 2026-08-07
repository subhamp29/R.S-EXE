/**
 * predev — kill any stale process still listening on the Vite dev port (1420)
 * so `tauri dev` doesn't fail with "Port 1420 is already in use".
 *
 * This runs automatically whenever `npm run dev` is invoked, including when
 * Tauri's `beforeDevCommand` calls it. It is safe to run before every dev session:
 * port 1420 is dedicated to this project's Vite server, so any process bound to
 * it is almost certainly a stray Vite instance from a previous session.
 *
 * Cross-platform:
 *   - Windows: uses `netstat -ano | findstr :1420` to find the PID, then
 *     `Stop-Process -Id <pid> -Force` via PowerShell (more reliable than
 *     taskkill, which can return "Not found" for processes it cannot see)
 *   - macOS / Linux: uses `lsof -ti:1420 -sTCP:LISTEN` to find the PID, then `kill -9`
 *
 * If no process is found (or the port is only in TIME_WAIT with no owner),
 * the script exits silently with code 0 so Vite can proceed normally.
 */
const { execSync } = require('child_process');
const os = require('os');

const PORT = 1420;

if (require.main === module) {
  main();
}

function main() {
  const platform = os.platform();
  const pids = findPidsOnPort(platform, PORT);

  if (pids.length === 0) {
    // Port is free — nothing to do.
    console.log(`[predev] Port ${PORT} is free, starting Vite.`);
    process.exit(0);
  }

  console.warn(`[predev] Port ${PORT} is in use by PID(s): ${pids.join(', ')}`);

  let killed = 0;
  for (const pid of pids) {
    const result = killPid(platform, pid);
    if (result.alreadyGone) {
      console.log(`[predev] PID ${pid} was already gone (port ${PORT} is free).`);
      killed++;
    } else if (result.ok) {
      console.log(`[predev] Killed stale process ${pid} on port ${PORT}.`);
      killed++;
    } else {
      console.error(
        `[predev] Could not kill PID ${pid} — kill command failed (see error output above).`,
      );
    }
  }

  if (killed === 0) {
    console.error(
      `[predev] Port ${PORT} is still in use and could not be freed.\n` +
      '        Vite will likely fail with "Port 1420 is already in use".\n' +
      '        To fix manually:\n' +
      `          Windows:  Stop-Process -Id <pid> -Force  (in PowerShell)\n` +
      `          macOS/Linux: lsof -ti:${PORT}  then  kill -9 <pid>`
    );
    process.exit(1);
  }

  // Give the OS a moment to release the socket before Vite binds.
  const delay = 500;
  const start = Date.now();
  // Synchronous busy-wait — predev is short-lived, this is fine.
  while (Date.now() - start < delay) {
    /* spin */
  }

  console.log(`[predev] Stale process(es) cleared on port ${PORT}. Starting Vite.`);
  process.exit(0);
}

/**
 * Find PIDs of processes listening on `port`.
 * Returns a deduplicated list of PID strings.
 */
function findPidsOnPort(platform, port) {
  try {
    if (platform === 'win32') {
      // On Windows, netstat doesn't accept a port filter directly.
      // We pipe through findstr to isolate lines containing :<port>.
      // Output lines look like:
      //   TCP    0.0.0.0:1420   0.0.0.0:0   LISTENING       1234
      // The last whitespace-separated token is the PID.
      const out = execSync(`netstat -ano | findstr :${port}`, {
        encoding: 'utf8',
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      const pids = new Set();
      for (const line of out.split(/\r?\n/)) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        // Only match LISTENING (stale processes) — skip TIME_WAIT and other states
        // since those are transient and will clear on their own.
        if (!trimmed.includes('LISTENING')) continue;
        const parts = trimmed.split(/\s+/);
        const pid = parts[parts.length - 1];
        if (/^\d+$/.test(pid)) pids.add(pid);
      }
      return [...pids];
    } else {
      // macOS / Linux: lsof returns one PID per line.
      const out = execSync(`lsof -ti:${port} -sTCP:LISTEN`, {
        encoding: 'utf8',
        stdio: ['pipe', 'pipe', 'pipe'],
      });
      const pids = new Set();
      for (const line of out.split(/\r?\n/)) {
        const pid = line.trim();
        if (/^\d+$/.test(pid)) pids.add(pid);
      }
      return [...pids];
    }
  } catch (e) {
    // Command failed — either no process on the port (exit code 1 from netstat/lsof),
    // or the tool isn't available. Either way, nothing to kill.
    return [];
  }
}

/**
 * Kill a process by PID. Returns an object:
 *   { ok: true, alreadyGone: false } — process was killed successfully
 *   { ok: true, alreadyGone: true }  — process was already gone (no action needed)
 *   { ok: false }                    — kill command failed (permissions, etc.)
 *
 * Logs raw stderr/stdout from the kill command for diagnostics.
 * When the kill command fails, the error is reported clearly so future
 * failures are actually diagnosable (not silently swallowed).
 */
function killPid(platform, pid) {
  try {
    if (platform === 'win32') {
      // Use Stop-Process via PowerShell instead of taskkill.
      // taskkill can return "ERROR: Not found" for processes it cannot see or
      // kill (e.g. processes in a different session or with elevated privileges),
      // which leads to mislabeling a live process as "already gone".
      // Stop-Process -Force is more reliable for force-killing on Windows.
      execSync(
        `powershell -Command "Stop-Process -Id ${pid} -Force"`,
        { encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] },
      );
    } else {
      execSync(`kill -9 ${pid}`, { encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
    }
    return { ok: true, alreadyGone: false };
  } catch (e) {
    const stderr = (e.stderr || '').trim();
    const stdout = (e.stdout || '').trim();
    const status = e.status;

    // On Windows, Stop-Process returns exit code 1 with an error message
    // when the process has already exited. That's fine — the port is free.
    if (platform === 'win32' && status === 1) {
      const combinedOutput = (stderr + ' ' + stdout).toLowerCase();
      if (
        combinedOutput.includes('cannot find a process') ||
        combinedOutput.includes('noprocessfoundforgivenid') ||
        combinedOutput.includes('not found')
      ) {
        console.log(
          `[predev] Stop-Process -Id ${pid} -Force: process already gone (port is free).`,
        );
        return { ok: true, alreadyGone: true };
      }
    }

    // On non-Windows platforms, kill -9 returns exit code 1 with "No such process"
    // when the target has already exited.
    if (platform !== 'win32' && status === 1) {
      if (stderr.includes('No such process') || stderr.includes('not found')) {
        console.log(`[predev] kill -9 ${pid}: process already gone (port is free).`);
        return { ok: true, alreadyGone: true };
      }
    }

    // Log the raw kill-command output so failures are diagnosable.
    // Differentiate between "process already gone" (fine) and
    // "kill command failed/denied" (needs attention).
    if (platform === 'win32') {
      console.error(
        `[predev] Stop-Process -Id ${pid} -Force failed (exit code ${status}).`,
      );
    } else {
      console.error(`[predev] kill -9 ${pid} failed (exit code ${status}).`);
    }
    if (stderr) console.error(`[predev] kill stderr: ${stderr}`);
    if (stdout) console.error(`[predev] kill stdout: ${stdout}`);
    return { ok: false, alreadyGone: false };
  }
}
