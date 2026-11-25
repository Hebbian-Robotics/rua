# Security policy

This software drives real robot arms. Treat every security problem as a safety
problem too.

## Deployment guidance

The station's HTTP control API has no authentication. Anyone who can reach it can
enable, move and stop the arms and read camera frames. Therefore:

- Keep the default loopback addresses. Do not bind `--http-address` or
  `--media-address` to an untrusted network or the public internet.
- For remote access, use a network you trust and control and protect every
  exposed endpoint with your own authentication. The station API can start
  recordings, report cameras for a lease, and issue camera tokens. The viewer
  backend also exposes state and command APIs to the browser.
  Treat remote viewer users as
  robot operators, not anonymous observers.
- Keep a physical power cutoff within reach whenever an arm is powered.
- Store LiveKit credentials in the station user’s LiveKit CLI configuration,
  the environment, or the git-ignored `.env` file. Keep other API keys in the
  environment or `.env` as well.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Open a private security
advisory on this repository's GitHub "Security" tab. Include the version or commit,
what you ran, and what an attacker could do. We will acknowledge the report and
keep you informed while we fix it.

## Supported versions

Only the latest commit on the default branch receives fixes.
