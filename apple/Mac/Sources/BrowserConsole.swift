import WebKit
import WheelKit

/// Forwards a browser pane's console to the app. The script runs at document start in every
/// frame; the top frame's first message says the page just loaded, so the daemon drops what it
/// kept for the pane.
enum BrowserConsole {
    static let handlerName = "wheelConsole"

    static let script = #"""
    (function () {
      if (window.__wheelConsole) return;
      window.__wheelConsole = true;
      function post(message) {
        try { window.webkit.messageHandlers.wheelConsole.postMessage(message); } catch (e) {}
      }
      if (window === window.top) post({ reset: true });
      function show(value) {
        try {
          if (typeof value === 'string') return value;
          if (value instanceof Error) return value.name + ': ' + value.message;
          var json = JSON.stringify(value);
          return json === undefined ? String(value) : json;
        } catch (e) { return String(value); }
      }
      function where(stack) {
        var frames = String(stack || '').split('\n');
        for (var i = 0; i < frames.length; i++) {
          var m = /([^@()\s]+):(\d+):\d+\)?\s*$/.exec(frames[i]);
          if (m && m[1].indexOf('wheelConsole') < 0) return { url: m[1], line: Number(m[2]) };
        }
        return {};
      }
      ['log', 'info', 'warn', 'error', 'debug'].forEach(function (level) {
        var original = console[level];
        console[level] = function () {
          var args = Array.prototype.slice.call(arguments);
          var at = where(new Error().stack.split('\n').slice(1).join('\n'));
          post({ time: Date.now(), level: level, text: args.map(show).join(' ').slice(0, 4096), url: at.url, line: at.line });
          return original.apply(console, arguments);
        };
      });
      window.addEventListener('error', function (e) {
        post({ time: Date.now(), level: 'error', text: 'Uncaught ' + (e.error ? show(e.error) : e.message),
               url: e.filename || undefined, line: e.lineno || undefined });
      });
      window.addEventListener('unhandledrejection', function (e) {
        var at = where(e.reason && e.reason.stack);
        post({ time: Date.now(), level: 'error', text: 'Unhandled promise rejection: ' + show(e.reason),
               url: at.url, line: at.line });
      });
    })();
    """#

    /// One message from the script: `reset` for a page load, else a console entry.
    static func decode(_ body: Any) -> (reset: Bool, entry: ConsoleEntry?)? {
        guard let body = body as? [String: Any] else { return nil }
        if body["reset"] as? Bool == true { return (true, nil) }
        guard let level = body["level"] as? String, let text = body["text"] as? String,
              let time = (body["time"] as? NSNumber)?.uint64Value else { return nil }
        return (false, ConsoleEntry(time: time, level: level, text: text,
                                    url: body["url"] as? String, line: (body["line"] as? NSNumber)?.uint32Value))
    }
}

/// Receives the script's messages without the web view's controller holding the pane.
final class ConsoleRelay: NSObject, WKScriptMessageHandler {
    var onMessage: (Bool, ConsoleEntry?) -> Void = { _, _ in }

    func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let decoded = BrowserConsole.decode(message.body) else { return }
        MainActor.assumeIsolated { onMessage(decoded.reset, decoded.entry) }
    }
}
