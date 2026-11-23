# Security

buzzard reads untrusted files. Report a vulnerability through GitHub private
vulnerability reporting: open the repository's "Security" tab and choose
"Report a vulnerability". Do not open a public issue for it.

A crafted `.vi`, `.ctl`, `.llb`, project, or `.tdms` file that makes buzzard
panic, hang, use memory without bound, or read or write outside the paths it
was given is a vulnerability. Include the file and the command you ran.

Fixes go into the latest release, or into `main` before the first release.
