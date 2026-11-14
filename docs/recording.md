# Recording

Install FFmpeg and put `ffmpeg` on `PATH` to enable recording. On Ubuntu:

```sh
sudo apt-get install ffmpeg
```

The media service uses this external command; the Python wheel does not bundle
it. Rust workspace integration tests also require it.

See [cameras](cameras.md) for camera setup. Playback and analysis tools are
coming soon.
