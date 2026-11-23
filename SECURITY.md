# Report a vulnerability

Do not open a public issue for a vulnerability. Report it through GitHub
private vulnerability reporting.

1. Open the repository's "Security" tab.
2. Choose "Report a vulnerability".
3. Include the file and the command that you ran.

The fix goes into the latest release, or into `main` before the first
release.

## What counts as a vulnerability

buzzard reads untrusted files. A crafted `.vi`, `.ctl`, `.llb`, project, or
`.tdms` file is a vulnerability when it makes buzzard panic, hang, use memory
without bound, or read or write outside the paths that you gave buzzard.
