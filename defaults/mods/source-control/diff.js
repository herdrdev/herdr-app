const UNSUPPORTED = "This change cannot be applied by line. Stage or unstage the whole file.";

export function parseDiff(text) {
  const diff = { headers: [], hunks: [], binary: false, unsupported: null };
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop();
  let hunk = null, oldLine = 0, newLine = 0, index = 0, files = 0;
  for (const raw of lines) {
    const match = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/.exec(raw);
    if (match) {
      hunk = { id: diff.hunks.length, header: raw, oldStart: +match[1], oldCount: match[2] === undefined ? 1 : +match[2], newStart: +match[3], newCount: match[4] === undefined ? 1 : +match[4], suffix: match[5], lines: [] };
      oldLine = hunk.oldStart; newLine = hunk.newStart; diff.hunks.push(hunk);
      continue;
    }
    if (raw.startsWith("diff --git ")) { files++; hunk = null; }
    if (!hunk) {
      diff.headers.push(raw);
      if (/^(Binary files |GIT binary patch)/.test(raw)) diff.binary = true;
      if (/^(new file mode |deleted file mode |old mode |new mode |rename |copy |similarity index |dissimilarity index |diff --cc |diff --combined |@@@|--- \/dev\/null|\+\+\+ \/dev\/null)/.test(raw)) diff.unsupported = UNSUPPORTED;
      const mode = /^index [0-9a-f]+\.\.[0-9a-f]+ (\d+)/.exec(raw)?.[1];
      if (mode && !["100644", "100755"].includes(mode)) diff.unsupported = UNSUPPORTED;
      continue;
    }
    const kind = raw[0];
    if (raw === "\\ No newline at end of file") {
      const previous = hunk.lines[hunk.lines.length - 1];
      if (!previous || previous.kind === "\\") diff.unsupported = UNSUPPORTED;
      else previous.noNewline = true;
      hunk.lines.push({ index: index++, kind: "\\", text: raw.slice(1), oldLine: null, newLine: null });
    } else if ([" ", "+", "-"].includes(kind)) {
      hunk.lines.push({ index: index++, kind, text: raw.slice(1), oldLine: kind === "+" ? null : oldLine++, newLine: kind === "-" ? null : newLine++ });
    } else diff.unsupported = UNSUPPORTED;
  }
  if (diff.binary || files !== 1 || !diff.headers.some(line => line.startsWith("--- ")) || !diff.headers.some(line => line.startsWith("+++ "))) diff.unsupported = UNSUPPORTED;
  for (const item of diff.hunks) {
    const oldCount = item.lines.filter(line => line.kind === " " || line.kind === "-").length;
    const newCount = item.lines.filter(line => line.kind === " " || line.kind === "+").length;
    if (oldCount !== item.oldCount || newCount !== item.newCount) diff.unsupported = UNSUPPORTED;
  }
  return diff;
}

// reverse builds an index-to-unstaged patch itself; do not pass git apply --reverse.
export function selectedPatch(diff, selectedLineIndexesOrSet, reverse = false) {
  if (diff.unsupported) throw new Error(diff.unsupported);
  const selected = new Set(selectedLineIndexesOrSet);
  const changed = new Set(diff.hunks.flatMap(hunk => hunk.lines.filter(line => line.kind === "+" || line.kind === "-").map(line => line.index)));
  if (!selected.size) throw new Error("Select changed lines to apply.");
  if ([...selected].some(index => !changed.has(index))) throw new Error("The selected lines are no longer valid. Refresh the diff.");
  const oldHeader = diff.headers.find(line => line.startsWith("--- "));
  const newHeader = diff.headers.find(line => line.startsWith("+++ "));
  const headers = diff.headers.filter(line => !line.startsWith("index ")).map(line => {
    if (!reverse) return line;
    if (line.startsWith("--- ")) return "--- " + newHeader.slice(4);
    if (line.startsWith("+++ ")) return "+++ " + oldHeader.slice(4);
    return line;
  });
  const output = [...headers];
  let offset = 0;
  for (const hunk of diff.hunks) {
    if (!hunk.lines.some(line => selected.has(line.index))) continue;
    const body = [], kept = [];
    let oldCount = 0, newCount = 0;
    for (const line of hunk.lines) {
      if (line.kind === "\\") continue;
      let kind = reverse ? line.kind === "+" ? "-" : line.kind === "-" ? "+" : " " : line.kind;
      if (kind !== " " && !selected.has(line.index)) {
        if (kind === "+") continue;
        kind = " ";
      }
      kept.push({ ...line, kind });
      if (kind !== "+") oldCount++;
      if (kind !== "-") newCount++;
    }
    let targetRemaining = newCount;
    for (const line of kept) {
      if (line.kind !== "-") targetRemaining--;
      // Inserting after an unterminated line needs a separator on the target side.
      if (line.kind === " " && line.noNewline && targetRemaining > 0) {
        body.push("-" + line.text, "\\ No newline at end of file", "+" + line.text);
      } else {
        body.push(line.kind + line.text);
        if (line.noNewline && (line.kind === "-" || targetRemaining === 0)) body.push("\\ No newline at end of file");
      }
    }
    const sourceCount = reverse ? hunk.newCount : hunk.oldCount;
    const sourceStart = reverse ? hunk.newStart : hunk.oldStart;
    const sourceIndex = sourceCount ? sourceStart - 1 : sourceStart;
    const oldStart = sourceIndex + (oldCount ? 1 : 0), newStart = sourceIndex + offset + (newCount ? 1 : 0);
    output.push(`@@ -${oldStart},${oldCount} +${newStart},${newCount} @@${hunk.suffix}`, ...body);
    offset += newCount - oldCount;
  }
  return output.join("\n") + "\n";
}
