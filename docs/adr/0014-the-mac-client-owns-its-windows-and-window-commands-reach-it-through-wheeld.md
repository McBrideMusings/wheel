# ADR-0014: The Mac client owns its windows, and window commands reach it through wheeld

A Mac window is a list of space ids, an optional name and a sidebar flag, kept in the Mac app's own preferences; wheeld stores no windows and the iPhone has none. A space is in one window or in the background, and closing a window moves its spaces to the background, where they keep running and are listed in the menu bar item. A `wheel window` command goes to wheeld, which forwards it to the attached Mac app and returns the app's reply; with no Mac app attached the command fails and says so.
