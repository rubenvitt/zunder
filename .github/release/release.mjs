// Runs semantic-release for release.yml and hands the result to the next
// jobs: tag, version and release notes go to $GITHUB_OUTPUT. semantic-release
// only computes the version from the Conventional Commits since the last tag
// and pushes the tag; the GitHub release itself is created by app.yml
// (tauri-action), the image tags by release.yml.
//
// Local dry run (no tag is pushed): node .github/release/release.mjs --dry-run
import { appendFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import semanticRelease from "semantic-release";

const preset = "conventionalcommits";
const presetConfig = {
  types: [
    { type: "feat", section: "Neu" },
    { type: "fix", section: "Behoben" },
    { type: "perf", section: "Schneller" },
    { type: "revert", section: "Zurückgenommen" },
  ],
};

const result = await semanticRelease({
  branches: ["main"],
  tagFormat: "v${version}",
  dryRun: process.argv.includes("--dry-run"),
  plugins: [
    ["@semantic-release/commit-analyzer", { preset, presetConfig }],
    ["@semantic-release/release-notes-generator", { preset, presetConfig }],
  ],
});

if (result && process.env.GITHUB_OUTPUT) {
  const { gitTag, version, notes } = result.nextRelease;
  const eof = `EOF_${randomUUID()}`;
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `tag=${gitTag}\nversion=${version}\nnotes<<${eof}\n${notes}\n${eof}\n`,
  );
}
