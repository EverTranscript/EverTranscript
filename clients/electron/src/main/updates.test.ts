import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "node:test";

import type { HoldAnswer, UpdateState } from "./update-state.js";
import { catchUpCore, createUpdates, isOlder, type UpdateSteps, type Updater } from "./updates.js";

/** Where the Windows download writes the installer. */
const SETUP = "C:\\Users\\o\\AppData\\Local\\evertranscript-updater\\pending\\EverTranscript-Setup-1.2.0.exe";

/** electron-updater, minus the network and the quitting. */
class FakeUpdater extends EventEmitter {
  autoDownload = true;
  autoInstallOnAppQuit = true;
  installs = 0;
  onInstall: () => void = () => {};
  checkForUpdates = async () => null;
  downloadUpdate = async () => [SETUP];
  quitAndInstall(silent: boolean, forceRun: boolean): void {
    assert.deepEqual([silent, forceRun], [true, true], "silent, and relaunched after");
    this.installs++;
    this.onInstall();
  }
}

/** A Core whose answers to `update/hold` are scripted, one per ask. */
function rig(answers: HoldAnswer[]) {
  const updater = new FakeUpdater();
  const published: UpdateState[] = [];
  const log = { holds: 0, releases: 0, pauses: 0, installers: [] as string[][] };
  const steps: UpdateSteps = {
    publish: (state) => published.push(state),
    hold: async (installers) => {
      log.holds++;
      log.installers.push(installers);
      const answer = answers.shift();
      assert.ok(answer, "asked the Core more often than the test scripted");
      return answer;
    },
    release: () => {
      log.releases++;
    },
    pause: async () => {
      log.pauses++;
    },
  };
  const updates = createUpdates(() => updater as unknown as Updater, steps);
  return { updater, updates, published, log, steps };
}

/** Found, and downloaded: where Restart to update is offered. */
async function downloaded(answers: HoldAnswer[]) {
  const it = rig(answers);
  it.updates.check();
  it.updater.emit("update-available", { version: "1.2.0" });
  await it.updates.download();
  assert.equal(it.updates.state().phase, "downloaded");
  return it;
}

test("checks never download or install on their own", () => {
  const { updater, updates } = rig([]);
  updates.check();
  assert.equal(updater.autoDownload, false);
  assert.equal(updater.autoInstallOnAppQuit, false);
});

test("a click while the Core is busy waits, then installs once it holds", async () => {
  const { updater, updates, published, log } = await downloaded(["recording", "diarizing", "held"]);
  await updates.install();
  assert.equal(updater.installs, 1);
  assert.equal(log.holds, 3);
  assert.equal(log.releases, 0, "a hold on the Core is kept until the installer runs");
  assert.deepEqual(log.installers, [[SETUP], [SETUP], [SETUP]], "every ask names the installer, so the hold outlives this Client");
  assert.deepEqual(
    published.slice(-3).map((state) => (state.phase === "waiting" ? state.busy : state.phase)),
    ["recording", "diarizing", "installing"],
  );
});

test("a second click while waiting does not install twice", async () => {
  const { updater, updates, steps } = await downloaded(["recording", "held"]);
  let resume = () => {};
  steps.pause = () => new Promise<void>((resolve) => (resume = resolve));
  const first = updates.install();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(updates.state().phase, "waiting");
  await updates.install();
  resume();
  await first;
  assert.equal(updater.installs, 1);
});

test("a Core that cannot say it is idle is never installed under", async () => {
  // A timeout, a refused connection, a Core too old for `update/hold`: none of
  // them is proof that nothing would be interrupted.
  const { updater, updates, steps } = await downloaded(["unreadable", "unreadable", "unreadable"]);
  let asked = 0;
  steps.pause = async () => {
    if (++asked === 3) throw new Error("gave up the test");
  };
  await assert.rejects(updates.install(), /gave up the test/);
  assert.equal(updater.installs, 0);
  assert.deepEqual(updates.state(), { phase: "waiting", version: "1.2.0", busy: "unreadable" });
});

test("a failed install hands the Core back, and Try again asks it again", async () => {
  const { updater, updates, log } = await downloaded(["held", "held"]);
  updater.onInstall = () => updater.emit("error", new Error("code signature did not pass validation"));
  await updates.install();
  assert.deepEqual(updates.state(), {
    phase: "failed",
    version: "1.2.0",
    retry: "install",
    message: "code signature did not pass validation",
  });
  assert.equal(log.releases, 1);

  updater.onInstall = () => {
    throw new Error("installer would not start");
  };
  await updates.install();
  assert.equal(log.holds, 2);
  assert.equal(log.releases, 2);
  assert.equal(updates.state().phase, "failed");
});

test("with no Core listening it installs, and a failure has no hold to hand back", async () => {
  const { updater, updates, log } = await downloaded(["absent"]);
  updater.onInstall = () => updater.emit("error", new Error("disk full"));
  await updates.install();
  assert.equal(updater.installs, 1);
  assert.equal(log.releases, 0);
  assert.equal(updates.state().phase, "failed");
});

test("a background error is not shown", async () => {
  const { updater, updates } = await downloaded([]);
  updater.emit("error", new Error("offline"));
  assert.equal(updates.state().phase, "downloaded");
});

/** A Core whose answers are scripted, for the catch-up. */
function catchUpRig(answers: HoldAnswer[]) {
  const log: string[] = [];
  return {
    log,
    steps: {
      hold: async () => {
        const answer = answers.shift();
        assert.ok(answer, "asked the Core more often than the test scripted");
        log.push(`hold:${answer}`);
        return answer;
      },
      restart: async () => {
        log.push("restart");
      },
      pause: async () => {
        log.push("pause");
      },
    },
  };
}

test("a Core of this Client's own version is left alone", async () => {
  const { log, steps } = catchUpRig([]);
  assert.equal(await catchUpCore("1.2.0", "1.2.0", steps), false);
  assert.deepEqual(log, []);
});

test("an older Core is restarted only under the hold, after it finishes recording", async () => {
  const { log, steps } = catchUpRig(["recording", "diarizing", "held"]);
  assert.equal(await catchUpCore("1.2.0-beta.1", "1.2.0-beta.2", steps), true);
  assert.deepEqual(log, ["hold:recording", "pause", "hold:diarizing", "pause", "hold:held", "restart"]);
});

test("a Core that cannot say it is idle is never restarted", async () => {
  // One older than `update/hold` among them: stopping it without the hold
  // could stop a recording, so it runs until the next login.
  for (const answer of ["unreadable", "absent"] as const) {
    const { log, steps } = catchUpRig([answer]);
    assert.equal(await catchUpCore("1.1.2", "1.2.0", steps), false);
    assert.deepEqual(log, [`hold:${answer}`]);
  }
});

test("only an older Core counts as older, in semver order, betas included", () => {
  const older: Array<[string, string]> = [
    ["1.1.2", "1.2.0"],
    ["1.1.2", "1.2.0-beta.1"], // a stable Core, a Client from the next line's beta
    ["1.2.0-beta.1", "1.2.0-beta.2"],
    ["1.2.0-beta.2", "1.2.0-beta.10"], // numeric, not lexical
    ["1.2.0-beta.2", "1.2.0"], // the beta a stable release replaces
    ["1.2.0", "1.10.0"],
  ];
  for (const [core, client] of older) {
    assert.equal(isOlder(core, client), true, `${core} < ${client}`);
    assert.equal(isOlder(client, core), false, `${client} is not < ${core}`);
  }
  assert.equal(isOlder("1.2.0", "1.2.0"), false);
  assert.equal(isOlder("1.2.0-beta.1", "1.2.0-beta.1"), false);
});

test("a newer, equal or unparseable Core is never restarted", async () => {
  const never: Array<[string, string]> = [
    ["1.2.0", "1.1.2"], // an older Client, a rollback: it must not replace a newer Core
    ["1.2.0", "1.2.0-beta.2"],
    ["1.2.0-beta.2", "1.2.0-beta.1"],
    ["1.2.0", "1.2.0"],
    ["", "1.2.0"],
    ["unknown", "1.2.0"],
    ["1.2", "1.3.0"], // not semver, so nothing says which side is older
    ["1.1.2", "garbage"],
  ];
  for (const [core, client] of never) {
    // An empty script fails the test if the Core is even asked for a hold.
    const { log, steps } = catchUpRig([]);
    assert.equal(await catchUpCore(core, client, steps), false, `${core} vs ${client}`);
    assert.deepEqual(log, [], `${core} vs ${client}`);
  }
});
