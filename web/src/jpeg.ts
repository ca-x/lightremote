/** Inspect JPEG marker metadata only. Never allocate or decode the pixel raster here. */
export const MAX_JPEG_DIMENSION = 16_384;
export const MAX_JPEG_PIXELS = 16_777_216;
export interface JPEGDimensions {
  width: number;
  height: number;
}

export function jpegDimensions(bytes: Uint8Array): JPEGDimensions {
  const invalid = (): never => {
    throw new Error("Invalid or truncated JPEG markers");
  };
  if (
    bytes.length < 4 ||
    bytes.length > 8 * 1024 * 1024 ||
    bytes[0] !== 0xff ||
    bytes[1] !== 0xd8
  )
    invalid();
  let offset = 2,
    inScan = false,
    sawScan = false,
    components = 0;
  let dimensions: JPEGDimensions | undefined;
  while (offset < bytes.length) {
    // Walk entropy bytes only to find subsequent markers, without decoding them.
    if (inScan)
      while (offset < bytes.length && bytes[offset] !== 0xff) offset++;
    if (offset >= bytes.length || bytes[offset++] !== 0xff) invalid();
    while (offset < bytes.length && bytes[offset] === 0xff) offset++;
    if (offset >= bytes.length) invalid();
    const marker = bytes[offset++];
    if (inScan && (marker === 0 || (marker >= 0xd0 && marker <= 0xd7)))
      continue;
    inScan = false;
    if (marker === 0xd9) {
      if (!dimensions || !sawScan || offset !== bytes.length) invalid();
      return dimensions!;
    }
    // Reject nested frames, misplaced standalone markers, and DNL height overrides.
    if (
      marker < 0xc0 ||
      marker === 0xd8 ||
      marker === 0xdc ||
      (marker >= 0xd0 && marker <= 0xd7)
    )
      invalid();
    if (offset + 2 > bytes.length) invalid();
    const length = bytes[offset] * 256 + bytes[offset + 1];
    if (length < 2 || offset + length > bytes.length) invalid();
    const body = offset + 2;
    const sof =
      marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker);
    if (sof) {
      if (dimensions || length < 8) invalid();
      const height = bytes[body + 1] * 256 + bytes[body + 2];
      const width = bytes[body + 3] * 256 + bytes[body + 4];
      components = bytes[body + 5];
      if (
        components < 1 ||
        components > 4 ||
        length !== 8 + 3 * components ||
        ![8, 12, 16].includes(bytes[body])
      )
        invalid();
      if (
        !width ||
        !height ||
        width > MAX_JPEG_DIMENSION ||
        height > MAX_JPEG_DIMENSION ||
        width * height > MAX_JPEG_PIXELS
      )
        throw new Error("JPEG dimensions exceed desktop limits");
      dimensions = { width, height };
    } else if (marker === 0xda) {
      const scans = bytes[body];
      if (
        !dimensions ||
        length < 6 ||
        scans < 1 ||
        scans > components ||
        length !== 6 + 2 * scans
      )
        invalid();
      inScan = true;
      sawScan = true;
    }
    offset += length;
  }
  return invalid();
}
