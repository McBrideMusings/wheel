// Wheel's status reporter for pi: tells the daemon whether this pane's pi is working or idle.
// pi never asks for permission, so it never reports needs_you.
// @ts-nocheck

import { spawn } from "node:child_process";

function report(status: string): void {
  try {
    const child = spawn("wheel", ["pane", "report", status], {
      stdio: "ignore",
      detached: true,
    });
    child.on("error", () => {});
    child.unref();
  } catch {}
}

export default function (pi) {
  if (!process.env.WHEEL_PANE) {
    return;
  }
  pi.on("agent_start", () => report("working"));
  pi.on("agent_settled", (_event, ctx) => {
    if (ctx?.isIdle?.() === true) {
      report("idle");
    }
  });
}
