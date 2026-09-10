import { version } from '../../package.json';

/**
 * The console bundle's own release version.
 *
 * `web/package.json`'s `version` and the workspace crate version move together
 * via release-please (see CLAUDE.md, "Releases & versioning"), and the console
 * bundle and the `of-server` binary are built in the same `Dockerfile` into one
 * image — so this is exactly the version the accompanying server was built
 * from, with no network call and no possibility of drift between the two.
 */
export const APP_VERSION = version;
