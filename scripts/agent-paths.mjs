import { existsSync, lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, posix, win32 } from "node:path";
import { fileURLToPath } from "node:url";

const MODULE_REPOSITORY_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

function isInside(root, candidate, pathApi) {
  if (!root || !pathApi.isAbsolute(root) || !pathApi.isAbsolute(candidate)) return false;
  const relative = pathApi.relative(pathApi.resolve(root), pathApi.resolve(candidate));
  if (pathApi.isAbsolute(relative)) return false;
  return relative === "" || (relative !== ".." && !relative.startsWith(`..${pathApi.sep}`));
}

/**
 * Return the platform-standard persistent directory for the default agent.
 *
 * Identity material must not live in a repository checkout by default. The
 * explicit --data-dir flag remains available for operators that need a
 * different location (for example one directory per fleet worker).
 */
export function defaultAgentDataDir({
  platform = process.platform,
  env = process.env,
  home = homedir(),
  repositoryRoot = MODULE_REPOSITORY_ROOT,
} = {}) {
  const pathApi = platform === "win32" ? win32 : posix;
  if (!pathApi.isAbsolute(home)) {
    throw new Error("the home directory must be absolute for agent identity storage");
  }

  let base;
  if (platform === "darwin") {
    base = pathApi.join(home, "Library", "Application Support");
  } else if (platform === "win32") {
    base = pathApi.isAbsolute(env.APPDATA ?? "")
      ? env.APPDATA
      : pathApi.join(home, "AppData", "Roaming");
  } else {
    base = pathApi.isAbsolute(env.XDG_DATA_HOME ?? "")
      ? env.XDG_DATA_HOME
      : pathApi.join(home, ".local", "share");
  }

  const product = platform === "win32" || platform === "darwin" ? "Jeliya" : "jeliya";
  let destination = pathApi.join(base, product, "agents", "default");
  if (isInside(repositoryRoot, destination, pathApi)) {
    const fallbackBase = platform === "darwin"
      ? pathApi.join(home, "Library", "Application Support")
      : platform === "win32"
        ? pathApi.join(home, "AppData", "Roaming")
        : pathApi.join(home, ".local", "share");
    destination = pathApi.join(fallbackBase, product, "agents", "default");
  }
  if (isInside(repositoryRoot, destination, pathApi)) {
    throw new Error("the default agent identity directory resolves inside the repository");
  }
  return destination;
}

/**
 * Put a fail-safe ignore marker inside an agent data directory.
 *
 * This protects an explicitly selected directory even when an operator puts
 * it under a Git worktree with a name that the repository-level ignore rules
 * do not know. Existing markers are never overwritten.
 */
export function installAgentDataGitGuard(dataDir) {
  mkdirSync(dataDir, { recursive: true, mode: 0o700 });
  const marker = join(dataDir, ".gitignore");
  if (existsSync(marker)) {
    const stat = lstatSync(marker);
    if (stat.isSymbolicLink() || !stat.isFile()) {
      throw new Error(
        `agent data Git guard at ${marker} is not a regular file; refusing to start`,
      );
    }
    const rules = readFileSync(marker, "utf8")
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line !== "" && !line.startsWith("#"));
    // Require the exact effective rules. Merely finding these two lines is
    // insufficient because a later `!identity.secret` negation would reopen
    // the very path this guard is meant to close.
    if (rules.length !== 2 || rules[0] !== "*" || rules[1] !== "!.gitignore") {
      throw new Error(
        `agent data Git guard at ${marker} is not the exact deny-all policy; refusing to start`,
      );
    }
  } else {
    writeFileSync(
      marker,
      "# Jeliya agent state contains identity secrets. Never commit this directory.\n*\n!.gitignore\n",
      { encoding: "utf8", flag: "wx", mode: 0o600 },
    );
  }
  return marker;
}
