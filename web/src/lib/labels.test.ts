/**
 * `around` is the one function in this module a translator can break without
 * touching a type: a message is plain text, and nothing stops one from typing
 * the link marker twice.
 */

import { describe, expect, it } from 'vitest';

import { around, LINK } from './labels';

describe('around', () => {
  it('splits a sentence at the marker', () => {
    expect(around(`Read the${LINK}before continuing.`)).toEqual(['Read the', 'before continuing.']);
  });

  it('yields the whole sentence and an empty tail when the marker is missing', () => {
    expect(around('Read the docs before continuing.')).toEqual([
      'Read the docs before continuing.',
      ''
    ]);
  });

  it('keeps everything after the first marker, even a second one', () => {
    // A translation that accidentally repeats the marker must not lose the
    // rest of the sentence — only the text between the two anchors would be
    // wrong, which a reviewer can catch; a silently truncated sentence would
    // not be.
    expect(around(`Before${LINK}middle${LINK}after`)).toEqual(['Before', `middle${LINK}after`]);
  });
});
