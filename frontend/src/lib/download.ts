/**
 * How long the URL outlives the click. A browser may read the blob after the
 * click returns, and a URL revoked by then saves an empty or failed file.
 */
const READ_GRACE_MS = 60_000;

/**
 * Save a blob under `filename` through a transient object URL.
 *
 * A blob download keeps the API key out of the URL, unlike a plain link.
 * Stated once, so no export can forget to revoke the URL it created. The
 * anchor is in the document while it is clicked: Firefox follows no click on
 * a detached one.
 */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.hidden = true;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), READ_GRACE_MS);
}

/** The same, for a document serialised as readable JSON. */
export function downloadJson(data: unknown, filename: string): void {
  downloadBlob(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }), filename);
}
