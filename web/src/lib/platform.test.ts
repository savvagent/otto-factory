import { describe, expect, it, vi } from 'vitest';

import { discoverPlatformUrl, platformHome, platformLink } from './platform';

describe('platformLink', () => {
  it('points org-scoped pages at the org on the platform', () => {
    for (const page of ['members', 'teams', 'sso', 'usage'] as const) {
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

describe('platformHome', () => {
  it('accepts an absolute http(s) URL and drops trailing slashes', () => {
    expect(platformHome('https://otto.example')).toBe('https://otto.example');
    expect(platformHome('https://otto.example///')).toBe('https://otto.example');
    expect(platformHome('http://localhost:8080/')).toBe('http://localhost:8080');
  });

  it('keeps a path prefix', () => {
    expect(platformHome('https://x.example/otto/')).toBe('https://x.example/otto');
  });

  it('refuses anything that is not an absolute http(s) URL', () => {
    for (const bad of [
      undefined,
      '',
      '   ',
      '/relative',
      'otto.example',
      'javascript:alert(1)',
      'data:text/html,hi',
      'ftp://otto.example'
    ]) {
      expect(platformHome(bad)).toBeUndefined();
    }
  });
});

describe('discoverPlatformUrl', () => {
  const answer = (status: number, body: string) =>
    vi.fn(async () => new Response(body, { status })) as unknown as typeof fetch;

  it('reads the first authorization server from the protected-resource metadata', async () => {
    const fetcher = answer(
      200,
      JSON.stringify({ authorization_servers: ['https://otto.example/', 'https://other.example'] })
    );
    await expect(discoverPlatformUrl(fetcher)).resolves.toBe('https://otto.example');
    expect(fetcher).toHaveBeenCalledWith('/.well-known/oauth-protected-resource', {
      headers: { accept: 'application/json' }
    });
  });

  it('gives up quietly on anything else', async () => {
    const cases: (typeof fetch)[] = [
      answer(500, JSON.stringify({ authorization_servers: ['https://otto.example'] })),
      answer(200, 'not json'),
      answer(200, JSON.stringify({})),
      answer(200, JSON.stringify({ authorization_servers: [] })),
      answer(200, JSON.stringify({ authorization_servers: 'https://otto.example' })),
      answer(200, JSON.stringify({ authorization_servers: [42] })),
      answer(200, JSON.stringify({ authorization_servers: ['javascript:alert(1)'] })),
      answer(200, 'null'),
      vi.fn(async () => {
        throw new TypeError('network down');
      }) as unknown as typeof fetch
    ];
    for (const fetcher of cases) {
      await expect(discoverPlatformUrl(fetcher)).resolves.toBeUndefined();
    }
  });
});
