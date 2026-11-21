# Media distribution notices

Hebbian Robot Use media is MIT licensed; see `LICENSE` beside this file.

`licenses/rust/` contains a Rust dependency inventory and the available license
files. Entries without a file require review before release. The inventory also
includes build dependencies and may be broader than the linked binary.

`licenses/webrtc/LICENSE.md` is supplied with the WebRTC archive linked into
the media binary and contains its third-party notices.

The recorder embeds protobuf descriptors for `foxglove.CompressedVideo` and
`google.protobuf.Timestamp`. Their license texts are included under
`licenses/recording-schema/`; see the repository's third-party notices for
the source revisions used to reproduce the descriptor.

The Ubuntu media archive uses FFmpeg and other native libraries installed on
the target machine; it does not include those libraries. Their licenses and any
distribution obligations require separate review. In particular, an Ubuntu
FFmpeg build can enable GPL components; inspect the target's actual packages.
