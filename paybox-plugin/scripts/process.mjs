import { spawn } from "node:child_process";
import { ConformanceError } from "./report.mjs";

export async function withProcess(command, args, options, run, timeoutMs = 10_000) {
  const child = spawn(command, args, { ...options, detached: true, stdio: ["pipe", "pipe", "pipe"] });
  const closed = new Promise((resolve) => child.once("close", resolve));
  let rejectFailure;
  const failure = new Promise((_, reject) => { rejectFailure = reject; });
  const stop = (message, code) => rejectFailure(new ConformanceError(message, code));
  child.once("error", () => stop("Required executable could not start; check installation and permissions", "startup_failed"));
  child.stdin.on("error", () => stop("Process closed its input before the request completed", "startup_failed"));
  const interrupt = () => stop("Local check cancelled; child processes were stopped", "cancelled");
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  const timer = setTimeout(() => stop(`Process exceeded ${timeoutMs} ms`, "timeout"), timeoutMs);
  try {
    return await Promise.race([failure, run(child, closed, stop)]);
  } finally {
    clearTimeout(timer);
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
    if (child.pid) {
      try { process.kill(-child.pid, "SIGKILL"); }
      catch (error) { if (error.code !== "ESRCH") throw error; }
    }
    child.stdin.destroy();
    child.stdout.destroy();
    child.stderr.destroy();
    await closed;
  }
}
