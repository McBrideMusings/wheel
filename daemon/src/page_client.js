// Wheel's page-side channel: include with <script src=".wheel/client.js"></script>.
//   wheel.set(key, value)  store a JSON value under key; `wheel artifact state` reads it
//   wheel.onData(fn)       fn(value) for each value `wheel artifact data` pushes, the latest first
//   wheel.flag(id, verdict, note)  journal a usage-review verdict ("good" or "bad", note optional)
//                          as kind=review.flag; resolves when wheeld has taken it
//   wheel.run({command, cwd, env})  run a shell command in directory cwd (absolute) with the
//                          variables in env; resolves
//                          to {started, pid, exit?, output?, running?, log}
//   wheel.submit(value)    Send: store value (an object) under `submitted` and deliver it to what the
//                          artifact's entry names (`reply_to`, `webhook`); resolves to what each did
(() => {
  const base = location.pathname.replace(/^(\/artifacts\/[^/]+\/).*$/, "$1") + ".wheel/";
  const handlers = [];
  let latest;
  let have = false;
  const source = new EventSource(base + "events");
  source.onmessage = (event) => {
    latest = JSON.parse(event.data);
    have = true;
    handlers.forEach((fn) => fn(latest));
  };
  window.wheel = {
    async set(key, value) {
      const response = await fetch(base + "state/" + encodeURIComponent(key), {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(value),
      });
      if (!response.ok) throw new Error("wheel.set " + key + ": " + response.status + " " + (await response.text()));
    },
    async submit(value) {
      const response = await fetch(base + "submit", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(value),
      });
      if (!response.ok) throw new Error("wheel.submit: " + response.status + " " + (await response.text()));
      return response.json();
    },
    async flag(id, verdict, note) {
      const response = await fetch(base + "review-flag", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id, verdict, note }),
      });
      if (!response.ok) throw new Error("wheel.flag: " + response.status + " " + (await response.text()));
    },
    async run(spec) {
      const response = await fetch(base + "run", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(spec),
      });
      if (!response.ok) throw new Error("wheel.run: " + response.status + " " + (await response.text()));
      return response.json();
    },
    onData(fn) {
      handlers.push(fn);
      if (have) fn(latest);
    },
  };
})();
