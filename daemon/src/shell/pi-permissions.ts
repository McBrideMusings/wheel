// @ts-nocheck
// Wheel's permission modes for pi. pi runs every tool without asking; this extension asks first,
// through pi's own confirm dialog, which Wheel shows as a permission request. The mode comes from
// Wheel as the extension command /wheel-permissions <mode>, which pi runs without a model turn.
//   ask    ask before bash, edit, write and any tool pi or an extension adds
//   edits  ask before bash and added tools; edit and write run
//   never  ask nothing
const READ_ONLY = new Set(["read", "grep", "find", "ls"]);
const EDITS = new Set(["edit", "write"]);

export default function (pi) {
	let mode = "never";

	pi.registerCommand("wheel-permissions", {
		description: "Set Wheel's permission mode: ask, edits or never",
		handler: async (args) => {
			const next = String(args || "").trim();
			if (next !== "ask" && next !== "edits" && next !== "never") {
				throw new Error(`no permission mode ${next}; use ask, edits or never`);
			}
			mode = next;
		},
	});

	pi.on("tool_call", async (event, ctx) => {
		if (mode === "never" || READ_ONLY.has(event.toolName)) return;
		if (mode === "edits" && EDITS.has(event.toolName)) return;
		const input = event.input ?? {};
		const detail = input.command ?? input.path ?? input.file_path ?? JSON.stringify(input);
		const allowed = await ctx.ui.confirm(`Run ${event.toolName}`, String(detail));
		if (!allowed) return { block: true, reason: "The person denied this tool call." };
	});
}
