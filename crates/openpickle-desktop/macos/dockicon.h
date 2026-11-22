#pragma once
#ifdef __cplusplus
extern "C" {
#endif

/* Composites the avatar into a macOS-style rounded icon and assigns it to
   NSApplication.applicationIconImage (Dock and app switcher while running).
   Returns 0 on success. Path must be a readable image file. */
int openpickle_dock_icon_set(const char *path);

/* Restores the bundled app icon. Safe to call when no custom icon is set. */
void openpickle_dock_icon_reset(void);

/* Writes the composited 1024-point (2048-pixel) PNG to dest. Does not change
   the running app icon. Returns 0 on success. */
int openpickle_dock_icon_render(const char *src_path, const char *dest_path);

#ifdef __cplusplus
}
#endif
