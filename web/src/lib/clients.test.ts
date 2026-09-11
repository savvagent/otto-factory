/**
 * `CLIENTS` is a data table specifically so a new entry cannot silently
 * reintroduce the bug this test guards against: the connect page shows one
 * generic "replace the placeholder with a token" warning next to every
 * recipe's snippet, which is only true when the snippet actually has a
 * placeholder in it. A client whose token snippet declines to embed a secret
 * (see the `otto-cli` entry) must say so in its `note` — the only other place
 * `+page.svelte` renders anything for the reader — or the page tells the
 * reader to replace something that was never there.
 */

import { describe, expect, it } from 'vitest';

import { CLIENTS, PLACEHOLDER } from './clients';

describe('CLIENTS token recipes', () => {
  const url = 'https://mcp.example.test';
  const mintedToken = 'of_pat_realtoken';

  for (const recipe of CLIENTS) {
    it(`${recipe.id}'s token() either embeds a secret, or explains the alternative via note`, () => {
      const placeholderSnippet = recipe.token(url, '');
      const mintedSnippet = recipe.token(url, mintedToken);

      const embedsSecret =
        placeholderSnippet.includes(PLACEHOLDER) && mintedSnippet.includes(mintedToken);

      if (!embedsSecret) {
        expect(
          recipe.note,
          `${recipe.id} omits the token from its snippet but has no note`
        ).toBeDefined();
      }
    });
  }
});
