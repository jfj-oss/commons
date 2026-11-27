// The C side of mega's iPhone build: mega's own start (app/gpui/src/ios.rs) and gpui-mobile's entry points
// (app/gpui/vendor/gpui-mobile/src/ios/ffi.rs) that main.m uses.
#ifndef MEGA_IOS_H
#define MEGA_IOS_H
#include <stdint.h>
#include <stdbool.h>

void mega_ios_start(void);

void gpui_ios_will_enter_foreground(void *app_ptr);
void gpui_ios_did_become_active(void *app_ptr);
void gpui_ios_will_resign_active(void *app_ptr);
void gpui_ios_did_enter_background(void *app_ptr);
void gpui_ios_will_terminate(void *app_ptr);
bool gpui_ios_request_frame(void *window_ptr);
void gpui_ios_set_frame_waker(void *window_ptr, void (*waker)(void *context), void *context);
void *gpui_ios_get_window(void);
void gpui_ios_handle_open_url(void *url_ptr);

#endif
