#import <AppKit/AppKit.h>
#include "dockicon.h"

/* Apple's macOS 11+ icon grid: an 824 pt rounded square with a 185.4 pt
   corner radius, centered on a 1024 pt canvas (100 pt transparent margin),
   so the Dock tile matches the size of other app icons. */
static const CGFloat kIconPoints = 1024;
static const CGFloat kIconScale = 2;
static const CGFloat kPlatePoints = 824;
static const CGFloat kCornerRadius = 185.4;
/* The character is drawn at 90% of the canvas so it sits inside the plate. */
static const CGFloat kCharacterScale = 0.9;

static NSBitmapImageRep *OpenPickleDockIconRep(NSImage *avatar) {
  if (!avatar || avatar.size.width <= 0 || avatar.size.height <= 0) return nil;
  const NSInteger pixels = (NSInteger)(kIconPoints * kIconScale);
  NSBitmapImageRep *rep = [[NSBitmapImageRep alloc]
      initWithBitmapDataPlanes:NULL
                    pixelsWide:pixels
                    pixelsHigh:pixels
                 bitsPerSample:8
               samplesPerPixel:4
                      hasAlpha:YES
                      isPlanar:NO
                colorSpaceName:NSCalibratedRGBColorSpace
                   bytesPerRow:0
                  bitsPerPixel:0];
  if (!rep) return nil;
  rep.size = NSMakeSize(kIconPoints, kIconPoints);

  NSGraphicsContext *context = [NSGraphicsContext graphicsContextWithBitmapImageRep:rep];
  if (!context) return nil;
  [NSGraphicsContext saveGraphicsState];
  [NSGraphicsContext setCurrentContext:context];

  /* renderInContext: on a CALayer does not reliably apply cornerRadius, so
     the clip is a path. The avatar already has transparent margin; the white
     fill is the plate the character is drawn to sit on. */
  CGFloat inset = (kIconPoints - kPlatePoints) / 2;
  NSRect bounds = NSMakeRect(inset, inset, kPlatePoints, kPlatePoints);
  NSBezierPath *mask = [NSBezierPath bezierPathWithRoundedRect:bounds
                                                       xRadius:kCornerRadius
                                                       yRadius:kCornerRadius];
  [mask addClip];
  [[NSColor whiteColor] setFill];
  NSRectFill(bounds);

  NSSize source = avatar.size;
  /* The avatar art has its own transparent margin, so it is scaled against
     the canvas rather than the plate, then reduced slightly. */
  CGFloat fit = MAX(kIconPoints / source.width, kIconPoints / source.height) * kCharacterScale;
  NSSize drawn = NSMakeSize(source.width * fit, source.height * fit);
  NSRect dest = NSMakeRect((kIconPoints - drawn.width) / 2,
                           (kIconPoints - drawn.height) / 2,
                           drawn.width,
                           drawn.height);
  [avatar drawInRect:dest
            fromRect:NSZeroRect
           operation:NSCompositingOperationSourceOver
            fraction:1
      respectFlipped:YES
               hints:@{NSImageHintInterpolation : @(NSImageInterpolationHigh)}];

  [NSGraphicsContext restoreGraphicsState];
  return rep;
}

static NSImage *OpenPickleDockIconImage(NSImage *avatar) {
  NSBitmapImageRep *rep = OpenPickleDockIconRep(avatar);
  if (!rep) return nil;
  NSImage *icon = [[NSImage alloc] initWithSize:NSMakeSize(kIconPoints, kIconPoints)];
  [icon addRepresentation:rep];
  return icon;
}

static int OpenPickleDockIconWrite(NSImage *avatar, NSString *dest) {
  NSBitmapImageRep *rep = OpenPickleDockIconRep(avatar);
  if (!rep) return -1;
  NSData *data = [rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
  if (!data) return -2;
  return [data writeToFile:dest atomically:YES] ? 0 : -3;
}

/* The app applies the icon from the UI thread. Off the main thread, including
   `cargo test`, dispatch_sync onto the main queue would deadlock, so the AppKit
   assignment is queued instead. A missing NSApp (unit tests) is a no-op. */
static void OpenPickleDockIconOnMain(void (^work)(void)) {
  if (NSThread.isMainThread) {
    work();
  } else {
    dispatch_async(dispatch_get_main_queue(), work);
  }
}

int openpickle_dock_icon_set(const char *path) {
  if (!path) return -1;
  @autoreleasepool {
    NSString *source = [NSString stringWithUTF8String:path];
    if (source.length == 0) return -1;
    NSImage *avatar = [[NSImage alloc] initWithContentsOfFile:source];
    NSImage *icon = OpenPickleDockIconImage(avatar);
    if (!icon) return -2;
    OpenPickleDockIconOnMain(^{
      if (NSApp) [NSApp setApplicationIconImage:icon];
    });
    return 0;
  }
}

void openpickle_dock_icon_reset(void) {
  OpenPickleDockIconOnMain(^{
    if (NSApp) [NSApp setApplicationIconImage:nil];
  });
}

int openpickle_dock_icon_render(const char *src_path, const char *dest_path) {
  if (!src_path || !dest_path) return -1;
  @autoreleasepool {
    NSString *source = [NSString stringWithUTF8String:src_path];
    NSString *dest = [NSString stringWithUTF8String:dest_path];
    if (source.length == 0 || dest.length == 0) return -1;
    NSImage *avatar = [[NSImage alloc] initWithContentsOfFile:source];
    if (!avatar) return -2;
    return OpenPickleDockIconWrite(avatar, dest);
  }
}
