import { describe, expect, it } from 'vitest';
import { canonicalizeChecksum, checksumAlgoLabel } from './checksum';

const SHA256_ABC = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
const MD5_ABC = '900150983cd24fb0d6963f7d28e17f72';

describe('canonicalizeChecksum', () => {
  it('picks the algorithm from bare hex length', () => {
    expect(canonicalizeChecksum(SHA256_ABC)).toBe(`sha256:${SHA256_ABC}`);
    expect(canonicalizeChecksum(MD5_ABC)).toBe(`md5:${MD5_ABC}`);
  });

  it('lowercases uppercase hex', () => {
    expect(canonicalizeChecksum(SHA256_ABC.toUpperCase())).toBe(`sha256:${SHA256_ABC}`);
    expect(canonicalizeChecksum(`md5:${MD5_ABC.toUpperCase()}`)).toBe(`md5:${MD5_ABC}`);
  });

  it('accepts prefixed forms in any case', () => {
    for (const prefix of ['sha256:', 'SHA256:', 'Sha256:', 'sha-256:', 'SHA-256:']) {
      expect(canonicalizeChecksum(`${prefix}${SHA256_ABC}`)).toBe(`sha256:${SHA256_ABC}`);
    }
    for (const prefix of ['md5:', 'MD5:', 'Md5:']) {
      expect(canonicalizeChecksum(`${prefix}${MD5_ABC}`)).toBe(`md5:${MD5_ABC}`);
    }
  });

  it('tolerates surrounding whitespace', () => {
    expect(canonicalizeChecksum(`  sha256:  ${SHA256_ABC}  `)).toBe(`sha256:${SHA256_ABC}`);
    expect(canonicalizeChecksum(`  ${MD5_ABC}  `)).toBe(`md5:${MD5_ABC}`);
  });

  it('rejects garbage, wrong lengths and unknown algorithms', () => {
    expect(canonicalizeChecksum('')).toBeNull();
    expect(canonicalizeChecksum('   ')).toBeNull();
    expect(canonicalizeChecksum('abc')).toBeNull();
    expect(canonicalizeChecksum('a'.repeat(63))).toBeNull();
    expect(canonicalizeChecksum('a'.repeat(65))).toBeNull();
    expect(canonicalizeChecksum('a'.repeat(31))).toBeNull();
    expect(canonicalizeChecksum('g'.repeat(64))).toBeNull();
    expect(canonicalizeChecksum(`sha256:${MD5_ABC}`)).toBeNull();
    expect(canonicalizeChecksum(`md5:${SHA256_ABC}`)).toBeNull();
    expect(canonicalizeChecksum(`sha1:${'a'.repeat(40)}`)).toBeNull();
    expect(canonicalizeChecksum(`sha512:${'a'.repeat(128)}`)).toBeNull();
    expect(canonicalizeChecksum('sha256:')).toBeNull();
  });

  it('roundtrips canonical output', () => {
    const canonical = canonicalizeChecksum(SHA256_ABC);
    expect(canonicalizeChecksum(canonical!)).toBe(canonical);
  });
});

describe('checksumAlgoLabel', () => {
  it('labels canonical values for display', () => {
    expect(checksumAlgoLabel(`sha256:${SHA256_ABC}`)).toBe('SHA-256');
    expect(checksumAlgoLabel(`md5:${MD5_ABC}`)).toBe('MD5');
  });
});
