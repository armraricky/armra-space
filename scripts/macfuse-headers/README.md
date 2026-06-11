# macFUSE / libfuse build headers (vendored)

These are the FUSE API headers (libfuse, LGPL-2.1) that macFUSE installs at
`/usr/local/include/fuse`. They are needed only to **compile** rclone's `cmount`
(cgofuse compiles against `fuse.h`); cgofuse `dlopen`s the actual libfuse at
runtime, so no library is linked at build time. Vendored so CI can build the
patched rclone without installing the macFUSE cask. Not shipped in the app.
