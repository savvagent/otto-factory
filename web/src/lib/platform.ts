/**
 * Links into the otto platform's own console.
 *
 * Identity is not this app's: members, teams, SSO, usage, tokens, the account,
 * and the list of orgs all live at the platform, which is also what signed this
 * person in. The factory console links there rather than re-implementing any of
 * it. Every such link is built here, from the `platformUrl` the server reports
 * (`GET /api/session`), so a deployment's platform address is never baked into
 * the bundle and there is one place to fix a path when the platform's console
 * moves it.
 */

export type PlatformPage = 'members' | 'teams' | 'sso' | 'usage' | 'account' | 'orgs';

/** The platform console page for `page`, for the org `slug` where it is org-scoped. */
export function platformLink(platformUrl: string, page: PlatformPage, slug: string): string {
  const base = platformUrl.replace(/\/+$/, '');
  const org = `${base}/o/${encodeURIComponent(slug)}`;
  switch (page) {
    case 'members':
    case 'teams':
    case 'sso':
    case 'usage':
      return `${org}/${page}`;
    case 'account':
      return `${base}/settings`;
    case 'orgs':
      return base;
  }
}
