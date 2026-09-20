import { describe, it, expect, vi, afterEach } from 'vitest';
import { downloadBlob, downloadJson } from './download';

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('downloadBlob', () => {
  it('saves under the name it was given and lets go of the URL it made', () => {
    const create = vi.fn(() => 'blob:routarr/1');
    const revoke = vi.fn();
    vi.stubGlobal('URL', { ...URL, createObjectURL: create, revokeObjectURL: revoke });
    const clicked: HTMLAnchorElement[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      clicked.push(this);
    });

    downloadBlob(new Blob(['x']), 'routarr-logs.csv');

    expect(clicked).toHaveLength(1);
    const [anchor] = clicked;
    expect(anchor?.download).toBe('routarr-logs.csv');
    expect(anchor?.href).toBe('blob:routarr/1');
    expect(revoke).toHaveBeenCalledWith('blob:routarr/1');
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
