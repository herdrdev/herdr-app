#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <CoreVideo/CoreVideo.h>
#import <IOSurface/IOSurface.h>
#import <CoreText/CoreText.h>
#include <stdio.h>
#include <stdarg.h>
extern void herdr_renderer_report_error(const char *message);

@interface HerdrMetalView : NSView {
@public
    CVPixelBufferPoolRef copyPool;
    size_t poolWidth, poolHeight;
}
@end
@implementation HerdrMetalView
- (void)dealloc {
    if (copyPool) CVPixelBufferPoolRelease(copyPool);
    [super dealloc];
}
@end

// Detached layer-hosting view. GPUI composites its frames, so it never covers
// popovers or intercepts events. These functions are called on the UI thread.
void *herdr_metal_view_new(void) {
    NSView *view = [[HerdrMetalView alloc] initWithFrame:NSMakeRect(0, 0, 1, 1)];
    return view;
}
void herdr_metal_view_free(void *raw) { [(NSView *)raw release]; }
void herdr_metal_view_resize(void *raw, uint32_t width, uint32_t height, double scale) {
    NSView *view = raw;
    NSRect frame = NSMakeRect(0, 0, width / scale, height / scale);
    if (!NSEqualRects(view.frame, frame)) view.frame = frame;
    view.layer.contentsScale = scale;
}

static void *copy_error(char *message, size_t capacity, const char *format, ...) {
    va_list args; va_start(args, format);
    vsnprintf(message, capacity, format, args);
    va_end(args); return NULL;
}

void *herdr_metal_copy_frame(void *raw, void *rawDevice, void *rawQueue, char *message, size_t capacity) {
    @autoreleasepool {
        HerdrMetalView *view = raw;
        id contents = view.layer.contents;
        if (!contents || CFGetTypeID((CFTypeRef)contents) != IOSurfaceGetTypeID()) return copy_error(message, capacity, "Ghostty produced no IOSurface");
        IOSurfaceRef source = (IOSurfaceRef)contents;
        size_t width = IOSurfaceGetWidth(source), height = IOSurfaceGetHeight(source);
        if (!width || !height || IOSurfaceGetPixelFormat(source) != kCVPixelFormatType_32BGRA) return copy_error(message, capacity, "Ghostty produced an invalid BGRA surface");
        // Reuse Ghostty's chosen GPU and command queue, including on Intel.
        id<MTLDevice> device = (id<MTLDevice>)rawDevice;
        id<MTLCommandQueue> queue = (id<MTLCommandQueue>)rawQueue;
        if (view->copyPool && (view->poolWidth != width || view->poolHeight != height)) {
            CVPixelBufferPoolRelease(view->copyPool); view->copyPool = NULL;
        }
        if (!view->copyPool) {
            NSDictionary *attrs = @{
                (id)kCVPixelBufferWidthKey: @(width), (id)kCVPixelBufferHeightKey: @(height),
                (id)kCVPixelBufferPixelFormatTypeKey: @(kCVPixelFormatType_32BGRA),
                (id)kCVPixelBufferIOSurfacePropertiesKey: @{},
                (id)kCVPixelBufferMetalCompatibilityKey: @YES
            };
            CVReturn result = CVPixelBufferPoolCreate(kCFAllocatorDefault, NULL, (CFDictionaryRef)attrs, &view->copyPool);
            if (result != kCVReturnSuccess) return copy_error(message, capacity, "CVPixelBufferPoolCreate: %d", result);
            view->poolWidth = width; view->poolHeight = height;
        }
        CVPixelBufferRef pixel = NULL;
        CVReturn result = CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, view->copyPool, &pixel);
        if (result != kCVReturnSuccess) return copy_error(message, capacity, "CVPixelBufferPoolCreatePixelBuffer: %d", result);
        // A separate GPU allocation prevents Ghostty reusing its triple-buffered
        // target while GPUI still has a previous frame in flight. No CPU readback.
        MTLTextureDescriptor *desc = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:width height:height mipmapped:NO];
#if defined(__arm64__)
        desc.storageMode = MTLStorageModeShared;
#else
        desc.storageMode = MTLStorageModeManaged;
#endif
        desc.usage = MTLTextureUsageShaderRead;
        id<MTLTexture> src = [device newTextureWithDescriptor:desc iosurface:source plane:0];
        id<MTLTexture> dst = [device newTextureWithDescriptor:desc iosurface:CVPixelBufferGetIOSurface(pixel) plane:0];
        id<MTLCommandBuffer> command = [queue commandBuffer];
        if (!src || !dst || !command) {
            [src release]; [dst release];
            CVPixelBufferRelease(pixel);
            return copy_error(message, capacity, "Metal blit texture/command allocation failed");
        }
        id<MTLBlitCommandEncoder> blit = [command blitCommandEncoder];
        if (!blit) {
            [src release]; [dst release]; CVPixelBufferRelease(pixel);
            return copy_error(message, capacity, "Metal blit encoder allocation failed");
        }
        [blit copyFromTexture:src sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
                 sourceSize:MTLSizeMake(width, height, 1) toTexture:dst destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
        [blit endEncoding];
        [command commit];
        [command waitUntilCompleted];
        BOOL ok = command.status == MTLCommandBufferStatusCompleted;
        [src release]; [dst release];
        if (!ok) { CVPixelBufferRelease(pixel); return copy_error(message, capacity, "Metal blit failed: %s", command.error.localizedDescription.UTF8String ?: "unknown GPU error"); }
        return pixel;
    }
}

// UI thread only. Source is pinned in the bridge, and immutable libraries can
// be reused across terminals and configuration changes on the same device.
void *herdr_metal_library(void *rawDevice, const uint8_t *source, size_t length) {
    @autoreleasepool {
        static NSMutableDictionary *libraries;
        if (!libraries) libraries = [[NSMutableDictionary alloc] init];
        id<MTLDevice> device = (id<MTLDevice>)rawDevice;
        NSNumber *key = @(device.registryID);
        id<MTLLibrary> library = libraries[key];
        if (!library) {
            NSString *text = [[NSString alloc] initWithBytes:source length:length encoding:NSUTF8StringEncoding];
            NSError *error = nil;
            library = [device newLibraryWithSource:text options:nil error:&error];
            [text release];
            if (!library) {
                herdr_renderer_report_error(error.localizedDescription.UTF8String ?: "Metal shader compilation failed");
                return NULL;
            }
            libraries[key] = library;
            [library release]; // the dictionary owns it
        }
        return [library retain]; // caller owns this retain
    }
}

bool herdr_renderer_register_font(const uint8_t *bytes, size_t length, char *message, size_t capacity) {
    @autoreleasepool {
        CFDataRef data = CFDataCreate(kCFAllocatorDefault, bytes, length);
        CFArrayRef descriptors = data ? CTFontManagerCreateFontDescriptorsFromData(data) : NULL;
        if (data) CFRelease(data);
        if (!descriptors || CFArrayGetCount(descriptors) == 0) {
            if (descriptors) CFRelease(descriptors);
            copy_error(message, capacity, "CoreText could not read font data"); return false;
        }
        bool ok = true;
        for (CFIndex i = 0; i < CFArrayGetCount(descriptors); i++) {
            CTFontRef font = CTFontCreateWithFontDescriptor(CFArrayGetValueAtIndex(descriptors, i), 12, NULL);
            CGFontRef graphics = font ? CTFontCopyGraphicsFont(font, NULL) : NULL;
            CFErrorRef error = NULL;
            // This synchronous API lets the next native renderer immediately
            // discover the font. Registration is process-scoped, never installed.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            bool registered = graphics && CTFontManagerRegisterGraphicsFont(graphics, &error);
#pragma clang diagnostic pop
            if (!registered && (!error || (CFErrorGetCode(error) != kCTFontManagerErrorAlreadyRegistered && CFErrorGetCode(error) != kCTFontManagerErrorDuplicatedName))) {
                CFStringRef detail = error ? CFErrorCopyDescription(error) : NULL;
                if (!detail || !CFStringGetCString(detail, message, capacity, kCFStringEncodingUTF8)) copy_error(message, capacity, "CoreText font registration failed");
                if (detail) CFRelease(detail);
                ok = false;
            }
            if (error) CFRelease(error);
            if (graphics) CFRelease(graphics);
            if (font) CFRelease(font);
        }
        CFRelease(descriptors);
        return ok;
    }
}

bool herdr_renderer_has_font(const char *family) {
    @autoreleasepool {
        NSString *name = [NSString stringWithUTF8String:family];
        // The system family-name inventory omits in-memory registrations.
        // Match through a descriptor collection, as Ghostty discovery does.
        NSDictionary *attributes = @{(id)kCTFontFamilyNameAttribute: name};
        CTFontDescriptorRef descriptor = CTFontDescriptorCreateWithAttributes((CFDictionaryRef)attributes);
        NSArray *descriptors = @[(id)descriptor];
        CTFontCollectionRef collection = CTFontCollectionCreateWithFontDescriptors((CFArrayRef)descriptors, NULL);
        CFArrayRef matches = CTFontCollectionCreateMatchingFontDescriptors(collection);
        bool found = matches && CFArrayGetCount(matches) > 0;
        if (matches) CFRelease(matches);
        CFRelease(collection); CFRelease(descriptor);
        return found;
    }
}

// Match GPUI's preference: non-removable, low-power GPUs before a discrete or
// external GPU. Both renderers therefore select the same adapter on Intel Macs.
void *herdr_metal_device(void) {
#if defined(__arm64__)
    return MTLCreateSystemDefaultDevice();
#else
    NSArray<id<MTLDevice>> *devices = MTLCopyAllDevices();
    id<MTLDevice> chosen = nil;
    unsigned best = UINT_MAX;
    for (id<MTLDevice> device in devices) {
        unsigned rank = (device.removable ? 2 : 0) + (device.lowPower ? 0 : 1);
        if (rank < best) { chosen = device; best = rank; }
    }
    [chosen retain]; [devices release];
    return chosen ?: MTLCreateSystemDefaultDevice();
#endif
}
