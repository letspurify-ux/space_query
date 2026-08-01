#import <AppKit/AppKit.h>

typedef struct {
  double x;
  double y;
  double width;
  double height;
} SpaceQueryMacWindowFrame;

int space_query_macos_capture_window_frame(
    void *raw_window,
    SpaceQueryMacWindowFrame *out_frame) {
  if (raw_window == NULL || out_frame == NULL) {
    return 0;
  }

  NSWindow *window = (NSWindow *)raw_window;
  NSRect frame = [window frame];
  out_frame->x = frame.origin.x;
  out_frame->y = frame.origin.y;
  out_frame->width = frame.size.width;
  out_frame->height = frame.size.height;
  return 1;
}

int space_query_macos_restore_window_frame(
    void *raw_window,
    const SpaceQueryMacWindowFrame *frame) {
  if (raw_window == NULL || frame == NULL) {
    return 0;
  }

  NSWindow *window = (NSWindow *)raw_window;
  NSRect native_frame =
      NSMakeRect(frame->x, frame->y, frame->width, frame->height);
  [window setFrame:native_frame display:YES animate:NO];
  return NSEqualRects([window frame], native_frame) ? 1 : 0;
}

int space_query_macos_window_is_zoomed(void *raw_window) {
  if (raw_window == NULL) {
    return 0;
  }
  return [(NSWindow *)raw_window isZoomed] ? 1 : 0;
}

int space_query_macos_window_is_fullscreen(void *raw_window) {
  if (raw_window == NULL) {
    return 0;
  }
  NSWindow *window = (NSWindow *)raw_window;
  return ([window styleMask] & NSWindowStyleMaskFullScreen) != 0 ? 1 : 0;
}

int space_query_macos_set_window_zoomed(void *raw_window, int zoomed) {
  if (raw_window == NULL) {
    return 0;
  }

  NSWindow *window = (NSWindow *)raw_window;
  BOOL target = zoomed != 0;
  if ([window isZoomed] != target) {
    NSWindowAnimationBehavior previous_behavior = [window animationBehavior];
    [window setAnimationBehavior:NSWindowAnimationBehaviorNone];
    [window performZoom:nil];
    [window setAnimationBehavior:previous_behavior];
  }
  return [window isZoomed] == target ? 1 : 0;
}
