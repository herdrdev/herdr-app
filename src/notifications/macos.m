// Native framework boundary: ARC owns Objective-C objects and copied blocks.
// All shared state and notification-center delegate work is on the main queue.
#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#include <stdbool.h>
#include <stdint.h>

typedef void (*HerdrNotificationCallback)(uint32_t, const char *);
static HerdrNotificationCallback callback;
static BOOL enabled;
static NSUInteger generation;
static NSMutableOrderedSet<NSString *> *identifiers;

static void report(NSString *message) {
    if (callback) callback(1, message.UTF8String);
}

@interface HerdrNotificationDelegate : NSObject <UNUserNotificationCenterDelegate>
@end
@implementation HerdrNotificationDelegate
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
      willPresentNotification:(UNNotification *)notification
        withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completion {
    // Background agents should also show a banner while Herdr is frontmost.
    // Audio remains the existing Herdr sound, rather than a second OS sound.
    completion(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList);
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
 didReceiveNotificationResponse:(UNNotificationResponse *)response
        withCompletionHandler:(void (^)(void))completion {
    if ([response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier]) {
        NSString *target = response.notification.request.content.userInfo[@"target"];
        if ([target isKindOfClass:NSString.class]) {
            dispatch_async(dispatch_get_main_queue(), ^{
                BOOL test = [response.notification.request.identifier hasSuffix:@".test"];
                if (callback && (enabled || test)) callback(test ? 2 : 0, target.UTF8String);
            });
        }
    }
    completion();
}
@end

static UNUserNotificationCenter *center(void) {
    static HerdrNotificationDelegate *delegate;
    // currentNotificationCenter raises an exception from an unbundled binary.
    NSBundle *bundle = NSBundle.mainBundle;
    if (!bundle.bundleIdentifier.length || ![bundle.bundleURL.pathExtension isEqualToString:@"app"]) return nil;
    UNUserNotificationCenter *result = UNUserNotificationCenter.currentNotificationCenter;
    if (!delegate) {
        delegate = [HerdrNotificationDelegate new];
        identifiers = [NSMutableOrderedSet new];
        result.delegate = delegate; // The center's delegate property is weak.
    }
    return result;
}

void herdr_notifications_configure(bool value, HerdrNotificationCallback handler) {
    callback = handler;
    if (enabled == value) return;
    enabled = value;
    NSUInteger current = ++generation;
    UNUserNotificationCenter *notifications = center();
    if (!value) {
        [notifications removePendingNotificationRequestsWithIdentifiers:identifiers.array ?: @[]];
        [notifications removeDeliveredNotificationsWithIdentifiers:identifiers.array ?: @[]];
        [identifiers removeAllObjects];
        return;
    }
    if (!notifications) {
        report(@"Launch Herdr.app to use system notifications; a bare binary cannot request permission.");
        return;
    }
    [notifications requestAuthorizationWithOptions:UNAuthorizationOptionAlert completionHandler:^(BOOL granted, NSError *error) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (!enabled || generation != current) return;
            if (error) report(error.localizedDescription);
            else if (!granted) report(@"Allow Herdr in System Settings > Notifications to receive system notifications.");
        });
    }];
}

void herdr_notifications_post(const char *identifier, const char *target, const char *title, const char *body, bool test) {
    @autoreleasepool {
        if (!enabled && !test) return;
        UNUserNotificationCenter *notifications = center();
        if (!notifications) {
            report(@"Launch Herdr.app to test system notifications.");
            return;
        }
        // Copy Rust's borrowed strings before any asynchronous callback.
        NSString *key = [NSString stringWithUTF8String:identifier];
        UNMutableNotificationContent *content = [UNMutableNotificationContent new];
        content.title = [NSString stringWithUTF8String:title];
        content.body = [NSString stringWithUTF8String:body];
        content.userInfo = @{@"target": [NSString stringWithUTF8String:target]};
        NSUInteger current = generation;
        void (^deliver)(void) = ^{
            dispatch_async(dispatch_get_main_queue(), ^{
                if (current != generation || (!enabled && !test)) return;
                [identifiers addObject:key];
                if (identifiers.count > 256) {
                    NSString *oldest = identifiers.firstObject;
                    [notifications removePendingNotificationRequestsWithIdentifiers:@[oldest]];
                    [notifications removeDeliveredNotificationsWithIdentifiers:@[oldest]];
                    [identifiers removeObjectAtIndex:0];
                }
                UNNotificationRequest *request = [UNNotificationRequest requestWithIdentifier:key content:content trigger:nil];
                [notifications addNotificationRequest:request withCompletionHandler:^(NSError *error) {
                    if (error) dispatch_async(dispatch_get_main_queue(), ^{
                        if (generation == current) report(error.localizedDescription);
                    });
                }];
            });
        };
        [notifications getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
            if (settings.authorizationStatus == UNAuthorizationStatusAuthorized || settings.authorizationStatus == UNAuthorizationStatusProvisional) {
                deliver();
            } else if (settings.authorizationStatus == UNAuthorizationStatusNotDetermined) {
                [notifications requestAuthorizationWithOptions:UNAuthorizationOptionAlert completionHandler:^(BOOL granted, NSError *error) {
                    if (granted) deliver();
                    else if (test) dispatch_async(dispatch_get_main_queue(), ^{
                        if (generation == current) report(error.localizedDescription ?: @"Allow Herdr in System Settings > Notifications to receive system notifications.");
                    });
                }];
            } else if (test) {
                dispatch_async(dispatch_get_main_queue(), ^{
                    if (generation == current) report(@"Allow Herdr in System Settings > Notifications to receive system notifications.");
                });
            }
        }];
    }
}
