// Wheel's page-side channel: include with <script src=".wheel/client.js"></script>.
//   wheel.set(key, value)  store a JSON value under key; `wheel artifact state` reads it
//   wheel.onData(fn)       fn(value) for each value `wheel artifact data` pushes, the latest first
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
    onData(fn) {
      handlers.push(fn);
      if (have) fn(latest);
    },
  };
})();
