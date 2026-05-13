import type { FileEntryWire } from "@/lib/wire-types";
import { formatBytes } from "@/lib/utils";

type FileViewerProps = {
  readonly file: FileEntryWire;
  readonly content: string | null;
};

const MAX_LINES = 5000;

export function FileViewer({ file, content }: FileViewerProps) {
  if (file.displayMode === "binary") {
    return (
      <FileShell file={file}>
        <NonText
          title="Binary file"
          message="The publish snapshot only retains metadata for binary files. Restore the repository locally to inspect content."
        />
      </FileShell>
    );
  }
  if (file.displayMode === "too_large") {
    return (
      <FileShell file={file}>
        <NonText
          title="File exceeds preview cap"
          message={`Configured publish.max_preview_bytes is below this file's size (${formatBytes(file.sizeBytes)}). Restore the repository locally to inspect content.`}
        />
      </FileShell>
    );
  }
  if (file.displayMode === "ignored") {
    return (
      <FileShell file={file}>
        <NonText
          title="Ignored by publish policy"
          message="This path matched a built-in deny rule or .librapublishignore entry and was excluded from the published snapshot."
        />
      </FileShell>
    );
  }

  const text = content ?? "";
  const lines = text.split("\n");
  const displayedLines = lines.slice(0, MAX_LINES);
  const truncated = lines.length > MAX_LINES;

  return (
    <FileShell file={file}>
      <pre
        className="libra-codebox overflow-x-auto text-sm leading-relaxed"
        aria-label={`Source of ${file.path}`}
      >
        <code>
          {displayedLines.map((line, idx) => (
            <span key={idx} className="block">
              <span
                className="select-none pr-4 inline-block w-12 text-right libra-text-faint"
                aria-hidden
              >
                {idx + 1}
              </span>
              {line.length === 0 ? "\n" : line}
            </span>
          ))}
        </code>
      </pre>
      {truncated && (
        <p className="mt-3 text-xs libra-text-faint">
          File preview truncated at {MAX_LINES.toLocaleString()} lines. Restore
          the repository locally to view the full content.
        </p>
      )}
    </FileShell>
  );
}

function FileShell({ file, children }: { readonly file: FileEntryWire; readonly children: React.ReactNode }) {
  return (
    <article>
      <header className="mb-3 flex flex-wrap items-baseline justify-between gap-3 text-sm">
        <div className="flex items-baseline gap-3">
          <h1 className="libra-mono font-semibold">{file.path}</h1>
          {file.language && <span className="libra-pill">{file.language}</span>}
        </div>
        <span className="libra-mono libra-text-muted tabular-nums">{formatBytes(file.sizeBytes)}</span>
      </header>
      {children}
    </article>
  );
}

function NonText({ title, message }: { readonly title: string; readonly message: string }) {
  return (
    <div className="libra-card libra-card-pad">
      <p className="text-sm font-medium">{title}</p>
      <p className="mt-1 text-sm libra-text-muted">{message}</p>
    </div>
  );
}
