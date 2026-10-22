// Wheel's status reporter for pi: tells the daemon whether this pane's pi is working or idle.
// pi never asks for permission, so it never reports needs_you. It also reports how full its
// context window is after every turn.
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

function reportContext(ctx): void {
  const usage = ctx?.getContextUsage?.();
  if (typeof usage?.tokens !== "number" || typeof usage?.contextWindow !== "number") {
    return;
  }
  try {
    const child = spawn(
      "wheel",
      ["pane", "report-context", "--used", String(usage.tokens), "--size", String(usage.contextWindow)],
      { stdio: "ignore", detached: true },
    );
    child.on("error", () => {});
    child.unref();
  } catch {}
}

export default function (pi) {
  if (!process.env.WHEEL_PANE) {
    return;
  }
  pi.on("agent_start", () => report("working"));
  pi.on("turn_end", (_event, ctx) => reportContext(ctx));
  pi.on("agent_settled", (_event, ctx) => {
    reportContext(ctx);
    if (ctx?.isIdle?.() === true) {
      report("idle");
    }
  });
}
