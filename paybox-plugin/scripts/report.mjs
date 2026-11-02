import release from "../starter-version.json" with { type: "json" };

export { release };

const LATEST_URL = "https://developer.paybox.sh/plugin-starter/latest.json";
const CHANGELOG_URL = "https://developer.paybox.sh/requirements.md#starter-changelog";

export class ConformanceError extends Error {
  constructor(message, code) {
    super(message);
    this.code = code;
  }
}

export function checkEnvironment() {
  if (process.versions.node.split(".")[0] !== "24" || !["darwin", "linux"].includes(process.platform)) {
    throw new ConformanceError("Use Node.js 24 on macOS or Linux (Windows: use WSL2)", "environment_invalid");
  }
}

export async function checkStarterUpdate(fetchLatest = () => fetch(LATEST_URL, { signal: AbortSignal.timeout(3000) })) {
  try {
    const response = await fetchLatest();
    if (!response.ok) return undefined;
    const { version } = await response.json();
    if (compareVersions(version, release.version) <= 0) return undefined;
    return { latest: version, changelog: CHANGELOG_URL };
  } catch {
    return undefined;
  }
}

function compareVersions(left, right) {
  const parse = (value) => /^\d+\.\d+\.\d+$/.test(value) ? value.split(".").map(Number) : undefined;
  const [a, b] = [parse(left), parse(right)];
  if (!a || !b) return 0;
  return a.map((part, index) => part - b[index]).find((difference) => difference !== 0) ?? 0;
}

export async function reportConformance(run) {
  const started = performance.now();
  let result;
  try {
    checkEnvironment();
    result = await run();
  } catch (error) {
    result = error instanceof ConformanceError
      ? { status: "failed", code: error.code, message: error.message }
      : { status: "failed", code: "local_check_failed", message: "Local checks could not complete; check files and build diagnostics" };
    process.exitCode = 1;
  }
  const durationMs = Math.round(performance.now() - started);
  const starterUpdate = await checkStarterUpdate();
  if (starterUpdate) {
    process.stderr.write(`Starter ${release.version} is out of date. Starter ${starterUpdate.latest} is available: ${starterUpdate.changelog}\n`);
  }
  process.stdout.write(`${JSON.stringify({ reportVersion: 1, starterVersion: release.version,
    protocolVersion: release.protocolVersion, toolchain: { node: process.versions.node, platform: process.platform, arch: process.arch },
    durationMs, ...(starterUpdate ? { starterUpdate } : {}), ...result })}\n`);
}
