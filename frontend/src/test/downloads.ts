import { vi } from 'vitest';

/**
 * What the page would save, by name and content. jsdom follows no download, so
 * the object URL and the anchor's click are stood in for and read back: the
 * name a reader would find in their downloads, and the blob behind it.
 * The caller unstubs the globals after the test.
 */
export function captureDownloads(): { name: string; blob: Blob }[] {
  const saved: { name: string; blob: Blob }[] = [];
  const blobs = new Map<string, Blob>();
  vi.stubGlobal('URL', {
    ...URL,
    createObjectURL: (blob: Blob) => {
      const url = `blob:test/${blobs.size}`;
      blobs.set(url, blob);
      return url;
    },
    revokeObjectURL: () => {},
  });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
    this: HTMLAnchorElement,
  ) {
    const blob = blobs.get(this.href);
    if (blob) saved.push({ name: this.download, blob });
  });
  return saved;
}
