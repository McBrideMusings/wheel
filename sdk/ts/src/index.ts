// Wheel's TypeScript SDK for plugin backends (docs/plugin-runtime.md, "Plugin backends").
//
//   const backend = new Backend();
//   backend.feed("now", (sub) => {
//     const timer = setInterval(() => sub.value({ time: new Date().toISOString() }), 1000);
//     return () => clearInterval(timer);          // runs when the last pane leaves
//   });
//   backend.action("echo", (call) => call.args);
//   backend.run();
//
// `run` talks to wheeld over the unix socket named by WHEEL_PLUGIN_SOCKET when it is set, else over
// standard input and output, with Content-Length framing (vscode-jsonrpc). Write logs with
// `peer.log`, never to standard output: that is the protocol's stream.

import * as net from "node:net";
import {
  createMessageConnection,
  MessageConnection,
  ResponseError,
  StreamMessageReader,
  StreamMessageWriter,
} from "vscode-jsonrpc/node";

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type LogLevel = "debug" | "info" | "warn" | "error";

/** One RFC 6902 JSON Patch operation. */
export interface PatchOp {
  op: "add" | "remove" | "replace" | "move" | "copy" | "test";
  path: string;
  from?: string;
  value?: Json;
}

/** Who invoked an action. */
export interface Caller {
  pane?: string;
  space?: string;
  plugin?: string;
  plugin_pane?: string;
}

/** The error wheeld answered a wheel/call with. */
export class CallError extends Error {
  constructor(public code: number, message: string) {
    super(message);
  }
}

/** Throw from an action to answer wheeld with a JSON-RPC error. */
export class ActionError extends Error {
  constructor(message: string, public code = -32000) {
    super(message);
  }
}

/** The connection to wheeld: logs and wheel/call. */
export class Peer {
  constructor(private readonly connection: MessageConnection) {}

  /** Write a line to the backend's log. */
  log(level: LogLevel, message: string): void {
    void this.connection.sendNotification("log", { level, message });
  }

  /** Run a Wheel operation (`pane.open`, `notify`, `command.run`) if the plugin's [backend] needs names it. */
  async call(method: string, args: Json = {}): Promise<Json> {
    try {
      return await this.connection.sendRequest("wheel/call", { method, args });
    } catch (e) {
      if (e instanceof ResponseError) throw new CallError(e.code, e.message);
      throw e;
    }
  }
}

/** One subscribed (feed, params). */
export class Subscription {
  constructor(
    readonly feed: string,
    readonly params: Json,
    readonly peer: Peer,
    private readonly connection: MessageConnection,
  ) {}

  /** Replace the feed's value. */
  value(value: Json): void {
    void this.connection.sendNotification("feed/update", { feed: this.feed, params: this.params, value });
  }

  /** Change the feed's last value by a JSON Patch. */
  patch(patch: PatchOp[]): void {
    void this.connection.sendNotification("feed/update", { feed: this.feed, params: this.params, patch });
  }
}

/** One action/invoke. */
export interface Call {
  action: string;
  args: Json;
  caller: Caller;
  peer: Peer;
}

/** Starts a feed for a subscription; the function it returns (sync or async) stops it. */
export type FeedHandler = (sub: Subscription) => void | (() => void | Promise<void>) | Promise<void | (() => void | Promise<void>)>;
export type ActionHandler = (call: Call) => Json | void | Promise<Json | void>;

const key = (feed: string, params: Json) => feed + JSON.stringify(params ?? {});

export class Backend {
  private feeds = new Map<string, FeedHandler>();
  private actions = new Map<string, ActionHandler>();
  private running = new Map<string, Promise<void | (() => void | Promise<void>)>>();

  /** Declare a feed. */
  feed(name: string, handler: FeedHandler): this {
    this.feeds.set(name, handler);
    return this;
  }

  /** Declare an action. Return its result (any JSON), or throw an ActionError. */
  action(name: string, handler: ActionHandler): this {
    this.actions.set(name, handler);
    return this;
  }

  /** Serve wheeld until it sends shutdown or the connection ends. */
  async run(): Promise<void> {
    const path = process.env.WHEEL_PLUGIN_SOCKET;
    if (path) {
      const socket = net.connect(path);
      await new Promise<void>((resolve, reject) => {
        socket.once("connect", resolve);
        socket.once("error", reject);
      });
      return this.serve(new StreamMessageReader(socket), new StreamMessageWriter(socket));
    }
    return this.serve(new StreamMessageReader(process.stdin), new StreamMessageWriter(process.stdout));
  }

  /** Serve over any reader and writer. Resolves when the connection ends or wheeld says shutdown. */
  serve(reader: StreamMessageReader, writer: StreamMessageWriter): Promise<void> {
    const connection = createMessageConnection(reader, writer);
    const peer = new Peer(connection);
    const done = new Promise<void>((resolve) => {
      connection.onClose(resolve);
      connection.onNotification("shutdown", () => {
        connection.dispose();
        resolve();
      });
    });
    connection.onRequest("initialize", (params: { plugin_id?: string }) => {
      peer.log("info", `initialized for ${params?.plugin_id ?? "?"}`);
      return { feeds: [...this.feeds.keys()], actions: [...this.actions.keys()] };
    });
    connection.onNotification("feed/subscribe", (r: { feed: string; params?: Json }) => {
      const handler = this.feeds.get(r.feed);
      if (!handler) return peer.log("warn", `subscribe to unknown feed "${r.feed}"`);
      const k = key(r.feed, r.params ?? {});
      void this.stop(k);
      const sub = new Subscription(r.feed, r.params ?? {}, peer, connection);
      this.running.set(k, Promise.resolve(handler(sub)));
    });
    connection.onNotification("feed/unsubscribe", (r: { feed: string; params?: Json }) => {
      void this.stop(key(r.feed, r.params ?? {}));
    });
    connection.onRequest("action/invoke", async (p: { action: string; args?: Json; caller?: Caller }) => {
      const handler = this.actions.get(p.action);
      if (!handler) throw new ResponseError(-32601, `no action "${p.action}"`);
      try {
        const result = await handler({ action: p.action, args: p.args ?? null, caller: p.caller ?? {}, peer });
        return result === undefined ? null : result;
      } catch (e) {
        if (e instanceof ActionError) throw new ResponseError(e.code, e.message);
        throw new ResponseError(-32000, e instanceof Error ? e.message : String(e));
      }
    });
    connection.listen();
    return done.then(() => this.stopAll());
  }

  private async stop(k: string): Promise<void> {
    const started = this.running.get(k);
    if (!started) return;
    this.running.delete(k);
    const stop = await started;
    if (typeof stop === "function") await stop();
  }

  private async stopAll(): Promise<void> {
    await Promise.all([...this.running.keys()].map((k) => this.stop(k)));
  }
}
