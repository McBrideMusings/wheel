// An example backend on the TypeScript SDK: the `ticker` plugin (wheel-plugin.toml beside this file).
// Feed `tick` sends {n, time} once a second; feed `log` sends {lines: []} then a JSON Patch appending
// "line <n>" each second; action `echo` returns {echo: args, caller}.

import { ActionError, Backend } from "../../src/index";

const backend = new Backend();

backend.feed("tick", (sub) => {
  let n = 0;
  const timer = setInterval(() => sub.value({ n: ++n, time: new Date().toISOString().slice(11, 19) }), 1000);
  return () => clearInterval(timer);
});

backend.feed("log", (sub) => {
  let n = 0;
  sub.value({ lines: [] });
  const timer = setInterval(() => sub.patch([{ op: "add", path: "/lines/-", value: `line ${++n}` }]), 1000);
  return () => clearInterval(timer);
});

backend.action("echo", (call) => ({ echo: call.args, caller: call.caller as Record<string, string> }));
backend.action("fail", () => {
  throw new ActionError("the ticker refuses");
});

backend.run().then(() => process.exit(0));
