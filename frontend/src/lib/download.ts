/**
 * Save a blob under `filename` through a transient object URL.
 *
 * A blob download keeps the API key out of the URL, unlike a plain link — and
 * four screens each wrote these six lines. Stated once, so the next export
 * cannot forget to revoke the URL it created.
 */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.click();
  URL.revokeObjectURL(url);
}

/** The same, for a document serialised as readable JSON. */
export function downloadJson(data: unknown, filename: string): void {
  downloadBlob(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' }), filename);
}
