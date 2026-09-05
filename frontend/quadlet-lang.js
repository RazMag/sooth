import { StreamLanguage } from "@codemirror/language";

// A tiny highlighter for quadlet / systemd-unit INI text: `[Section]` headers,
// `Key=Value` pairs, and `#`/`;` comments. Purpose-built (the grammar is
// trivial) rather than pulling in @codemirror/legacy-modes. Token names are
// @lezer/highlight tag names, resolved by StreamLanguage's default table.
export const quadletLanguage = StreamLanguage.define({
  name: "quadlet",
  startState: () => ({}),
  token(stream) {
    if (stream.sol()) {
      if (stream.match(/^\s*[#;].*/)) return "comment";
      if (stream.match(/^\s*\[[^\]]*]\s*$/)) return "heading";
      if (stream.match(/^\s*[A-Za-z][\w.-]*(?=\s*=)/)) return "propertyName";
    }
    if (stream.eat("=")) return "operator";
    if (stream.match(/[#;].*/)) return "comment";
    stream.skipToEnd();
    return null;
  },
});
