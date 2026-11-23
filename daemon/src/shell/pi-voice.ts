// @ts-nocheck
// Wheel's voice line for pi (ADR-0055). While voice mode talks to this agent, each run's system
// prompt carries the line as its own section; otherwise the section is gone. Wheel sends the line
// as the extension command /wheel-voice <line> and clears it with a bare /wheel-voice; pi runs the
// command without a model turn, even while a run streams.
export default function (pi) {
	let line = "";

	pi.registerCommand("wheel-voice", {
		description: "Set or clear Wheel's voice line",
		handler: async (args) => {
			line = String(args || "").trim();
		},
	});

	pi.on("before_agent_start", async (event) => {
		const sections = event.systemPromptOptions.sections;
		if (line) sections.wheel_voice = line;
		else delete sections.wheel_voice;
	});
}
