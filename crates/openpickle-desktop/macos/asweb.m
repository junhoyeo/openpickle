#import <AuthenticationServices/AuthenticationServices.h>
#import <AppKit/AppKit.h>
#include "asweb.h"
#include <string.h>

@interface OpenPickleASWebAnchor : NSObject <ASWebAuthenticationPresentationContextProviding>
@end
@implementation OpenPickleASWebAnchor
- (ASPresentationAnchor)presentationAnchorForWebAuthenticationSession:(ASWebAuthenticationSession *)session {
  (void)session;
  NSWindow *window = NSApp.keyWindow ?: NSApp.windows.firstObject;
  return window;
}
@end

// All state is confined to the main queue. A retained completion block may run
// after cancel/replacement, so each attempt has its own exactly-once gate.
@interface OpenPickleASWebAttempt : NSObject
@property(nonatomic) BOOL completed;
@property(nonatomic) openpickle_asweb_cb callback;
@property(nonatomic) void *context;
@end
@implementation OpenPickleASWebAttempt
@end

static ASWebAuthenticationSession *g_session = nil;
static OpenPickleASWebAnchor *g_anchor = nil;
static OpenPickleASWebAttempt *g_attempt = nil;

static void finish(OpenPickleASWebAttempt *attempt, const char *url, const char *error) {
  if (!attempt || attempt.completed) return;
  attempt.completed = YES;
  if (g_attempt == attempt) {
    g_session = nil;
    g_anchor = nil;
    g_attempt = nil;
  }
  attempt.callback(url, error, attempt.context);
}

int openpickle_asweb_start(const char *url, const char *callback_scheme, openpickle_asweb_cb cb, void *ctx) {
  if (!url || !callback_scheme || !cb) return 1;
  NSURL *nsurl = [NSURL URLWithString:[NSString stringWithUTF8String:url]];
  NSString *scheme = [NSString stringWithUTF8String:callback_scheme];
  if (!nsurl || scheme.length == 0) return 2;

  __block int status = 0;
  void (^startBlock)(void) = ^{
    openpickle_asweb_cancel();
    OpenPickleASWebAttempt *attempt = [OpenPickleASWebAttempt new];
    attempt.callback = cb;
    attempt.context = ctx;
    g_attempt = attempt;
    g_anchor = [OpenPickleASWebAnchor new];
    g_session = [[ASWebAuthenticationSession alloc]
        initWithURL:nsurl
        callbackURLScheme:scheme
        completionHandler:^(NSURL *callbackURL, NSError *error) {
          if (callbackURL != nil) {
            finish(attempt, callbackURL.absoluteString.UTF8String, NULL);
          } else if ([error.domain isEqualToString:ASWebAuthenticationSessionErrorDomain] &&
                     error.code == ASWebAuthenticationSessionErrorCodeCanceledLogin) {
            finish(attempt, NULL, "cancelled");
          } else {
            finish(attempt, NULL, error.localizedDescription.UTF8String ?: "authentication failed");
          }
        }];
    g_session.prefersEphemeralWebBrowserSession = NO;
    g_session.presentationContextProvider = g_anchor;
    if (![g_session start] && !attempt.completed) {
      // Nonzero return transfers context ownership back to the caller. Even a
      // late native completion must not access that context again.
      attempt.completed = YES;
      g_session = nil;
      g_anchor = nil;
      g_attempt = nil;
      status = 3;
    }
  };

  if ([NSThread isMainThread]) startBlock();
  else dispatch_sync(dispatch_get_main_queue(), startBlock);
  return status;
}

void openpickle_asweb_cancel(void) {
  void (^cancelBlock)(void) = ^{
    OpenPickleASWebAttempt *attempt = g_attempt;
    [g_session cancel];
    finish(attempt, NULL, "cancelled");
  };
  if ([NSThread isMainThread]) cancelBlock();
  else dispatch_sync(dispatch_get_main_queue(), cancelBlock);
}
