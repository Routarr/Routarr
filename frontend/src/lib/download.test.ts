import { describe, it, expect, vi, afterEach } from 'vitest';
import { downloadBlob, downloadJson } from './download';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('downloadBlob', () => {
  /**
   * The URL stays while the browser may still read the blob, and goes after:
   * revoked on the line after the click, some browsers save an empty file.
   */
  it('saves under the name it was given and lets go of the URL once read', () => {
    vi.useFakeTimers();
    const create = vi.fn(() => 'blob:routarr/1');
    const revoke = vi.fn();
    vi.stubGlobal('URL', { ...URL, createObjectURL: create, revokeObjectURL: revoke });
    const clicked: { anchor: HTMLAnchorElement; attached: boolean }[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push({ anchor: this, attached: this.isConnected });
    });

    try {
      downloadBlob(new Blob(['x']), 'routarr-logs.csv');

      expect(clicked).toHaveLength(1);
      const [{ anchor, attached }] = clicked as [(typeof clicked)[number]];
      expect(anchor.download).toBe('routarr-logs.csv');
      expect(anchor.href).toBe('blob:routarr/1');
      expect(attached).toBe(true);
      expect(anchor.isConnected).toBe(false);
      expect(revoke).not.toHaveBeenCalled();

      vi.advanceTimersByTime(60_000);
      expect(revoke).toHaveBeenCalledWith('blob:routarr/1');
    } finally {
      vi.useRealTimers();
    }
  });

  it('serialises JSON readably, as a JSON document', async () => {
    const blobs: Blob[] = [];
    vi.stubGlobal('URL', {
      ...URL,
      createObjectURL: (given: Blob) => {
        blobs.push(given);
        return 'blob:routarr/2';
      },
      revokeObjectURL: vi.fn(),
    });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});

    downloadJson({ a: 1 }, 'x.json');

    expect(blobs).toHaveLength(1);
    const [blob] = blobs;
    expect(blob?.type).toBe('application/json');
    expect(await blob?.text()).toBe('{\n  "a": 1\n}');
  });
});
