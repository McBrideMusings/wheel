# ADR-0048: A browser pane's Mac and phone browsers are independent

A browser pane has one WebKit browser on the Mac and one on the phone. Each has its own history, cookies, local storage and session storage. The page one is on, a login in one and a page's stored data pass to the other by no route, and the phone always draws its own page.

The only state the two share is the pane's home: the page the pane opens at, held in the layout as the pane's `url`. A browser opens at the home once and goes again only when the layout names another home for the pane (a `pane_set_kind` to a new page). It never reports where it goes, so wheeld holds no browser's current page and is sent none.

wheeld asks a browser on demand. It sends the one browser it wants, chosen by `on` (`mac`, the default, or `phone`), an `inspect_pane` over the path `eval`, `dom`, `shot` and `record` use (`ask_clients`), and the browser answers. `wheel pane page <pane> <page> --on mac|phone` is one such ask: wheeld resolves the text with `page_typed` and `page_allowed`, then sends `inspect_pane` with action `open` and the page as its argument, and the browser goes there and answers with no text. With no phone attached the ask fails with "the phone is not attached". An address field sends `pane_browser_open` with no `on`: wheeld only resolves the text and replies `opened`, and the browser the person typed in goes there itself.

The phone still loads a page on the Mac's loopback through wheeld's `/loopback/<port>/` (ADR-0005). The cookies of those pages live in the phone's own web view store, kept by `LoopbackCookies`, because WebKit gives a custom scheme none.

This replaces the model in which the page and the logins follow the pane. `PROTOCOL_VERSION` and `ServerProtocol.current` go to 47 together (ADR-0020).
