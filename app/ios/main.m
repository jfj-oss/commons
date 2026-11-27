// mega on iPhone: a UIKit app delegate around the Rust app (app/gpui/src/ios.rs). gpui-mobile makes the UIWindow and
// its view controller; this file starts it and drives frames with a CADisplayLink that pauses while nothing moves.
// Adapted from gpui-mobile's example host (Apache-2.0).
#import <UIKit/UIKit.h>
#import "mega_ios.h"

@interface MegaAppDelegate : UIResponder <UIApplicationDelegate>
@property (nonatomic, strong) UIWindow *window;
@property (nonatomic, assign) void *gpuiWindow;
@property (nonatomic, strong) CADisplayLink *displayLink;
@end

static void MegaResumeFrames(void *context);

@implementation MegaAppDelegate

- (BOOL)application:(UIApplication *)application didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    mega_ios_start();
    self.gpuiWindow = gpui_ios_get_window();
    if (!self.gpuiWindow) {
        NSLog(@"mega: no window was created");
        return YES;
    }
    [self startFrames];
    return YES;
}

- (void)startFrames {
    if (self.displayLink || !self.gpuiWindow) return;
    self.displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(renderFrame)];
    [self.displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
    gpui_ios_set_frame_waker(self.gpuiWindow, MegaResumeFrames, (__bridge void *)self);
}

- (void)renderFrame {
    // GPUI says after each frame whether it wants another; in between the link sleeps until the waker resumes it
    if (self.gpuiWindow && !gpui_ios_request_frame(self.gpuiWindow)) {
        self.displayLink.paused = YES;
    }
}

static void MegaResumeFrames(void *context) {
    MegaAppDelegate *delegate = (__bridge MegaAppDelegate *)context;
    delegate.displayLink.paused = NO;
}

- (void)applicationWillEnterForeground:(UIApplication *)application {
    gpui_ios_will_enter_foreground(NULL);
    [self startFrames];
}

- (void)applicationDidBecomeActive:(UIApplication *)application { gpui_ios_did_become_active(NULL); }
- (void)applicationWillResignActive:(UIApplication *)application { gpui_ios_will_resign_active(NULL); }

- (void)applicationDidEnterBackground:(UIApplication *)application {
    gpui_ios_did_enter_background(NULL);
    [self.displayLink invalidate];
    self.displayLink = nil;
}

- (BOOL)application:(UIApplication *)app openURL:(NSURL *)url options:(NSDictionary<UIApplicationOpenURLOptionsKey, id> *)options {
    gpui_ios_handle_open_url((__bridge void *)[url absoluteString]);
    return YES;
}

- (void)applicationWillTerminate:(UIApplication *)application {
    [self.displayLink invalidate];
    self.displayLink = nil;
    gpui_ios_will_terminate(NULL);
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([MegaAppDelegate class]));
    }
}
