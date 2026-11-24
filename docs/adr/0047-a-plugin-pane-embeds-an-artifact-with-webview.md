# ADR-0047: A plugin pane embeds an artifact with WebView{artifact}

A pane script shows an artifact inside its page with `ui.WebView{artifact = <name>, height?}`: the artifact's page as a web pane shows it, its `.wheel/wheel.css`, `.wheel/lightbox.js` and `.wheel/client.js` included, reloading when its files change.

`WebView` takes it, not a new component. The view is the same web view, as wide as it is offered and `height` points tall, with the same notes; only where the page comes from differs, and ADR-0031 adds a component only when nothing existing can carry it. A `WebView` naming both `url` and `artifact` draws a note.

The page is an artifact card's page under another owner. The client sends `plugin_embed {pane, artifact}` for the plugin pane showing it. `wheeld` refuses a pane that shows no plugin and a name the pane's space cannot resolve, resolving it as `wheel artifact` does (project, then machine, ad hoc artifacts included). Otherwise it keeps a page id for it beside the cards' (`page_artifact` looks in both), runs `show_artifact` on it as for a web pane, and replies with the id and the scope the name resolved in. So the page is served at `/artifacts/<id>/` behind the token, watched and refreshed, and `pane_reload` names the id, with no second route, watch or store. The same pane and artifact get the same id. A plugin pane keeps the 16 pages it asked for last, since each holds two folder watches and a feed of posts names many; the pages end when the pane closes or shows something else.

Each client loads the id through the page scheme it already has for web panes, asks for the page, shows it, reloads it, and asks again on each new connection, since a restarted `wheeld` knows no old id: the iPhone through WheelUI's `ArtifactEmbeds` and `paneEmbed`, the Mac in GPUI (ADR-0056).

The client relies on the new op, so `PROTOCOL_VERSION` and `ServerProtocol.current` go to 46 together.
