import { describe, expect, it } from 'vitest';

import { platformLink } from './platform';

describe('platformLink', () => {
  it('points org-scoped pages at the org on the platform', () => {
    for (const page of ['members', 'teams', 'sso', 'usage', 'tokens'] as const) {
      expect(platformLink('https://otto.example', page, 'acme')).toBe(
        `https://otto.example/o/acme/${page}`
      );
    }
  });

  it('points account and org switching at the platform itself', () => {
    expect(platformLink('https://otto.example', 'account', 'acme')).toBe(
      'https://otto.example/settings'
    );
    expect(platformLink('https://otto.example', 'orgs', 'acme')).toBe('https://otto.example');
  });

  it('survives a trailing slash and encodes the slug', () => {
    expect(platformLink('https://otto.example/', 'members', 'a b')).toBe(
      'https://otto.example/o/a%20b/members'
    );
  });
});
