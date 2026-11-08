//! Render Herdr's existing libghostty-vt state using Ghostty's Metal renderer.
//! All calls run on the UI thread; there is no second parser, PTY, or IO thread.
const std = @import("std");
const objc = @import("objc");
const global = @import("global.zig");
const renderer = @import("renderer.zig");
const font = @import("font/main.zig");
const Config = @import("config.zig").Config;
const terminal = @import("terminal/main.zig");
const c = terminal.c_api;
const alloc = std.heap.c_allocator;
pub const herdr_renderer = true;
pub const std_options = @import("main_ghostty.zig").std_options;
comptime {
    _ = @import("lib_vt.zig");
}

extern fn herdr_metal_view_new() ?*anyopaque;
extern fn herdr_metal_view_free(view: *anyopaque) void;
extern fn herdr_metal_view_resize(view: *anyopaque, width: u32, height: u32, scale: f64) void;
extern fn herdr_metal_copy_frame(view: *anyopaque, device: *anyopaque, queue: *anyopaque, message: [*]u8, capacity: usize) ?*anyopaque;

const CellSize = extern struct { width: u32, height: u32 };
const Renderer = struct {
    device: *renderer.Device,
    fonts: font.SharedGridSet,
    view: *anyopaque,
    metal: renderer.Renderer,
    scale: f64,
    fresh: bool = true,
};
// This internal API is UI-thread-only, including the borrowed error string.
var last_error: [1024:0]u8 = @splat(0);
pub export fn herdr_renderer_last_error() [*:0]const u8 {
    return &last_error;
}
pub export fn herdr_renderer_report_error(message: [*:0]const u8) void {
    const text = std.mem.span(message);
    const count = @min(text.len, last_error.len);
    @memcpy(last_error[0..count], text[0..count]);
    last_error[count] = 0;
}
fn recordError(err: anyerror) void {
    if (last_error[0] == 0) _ = std.fmt.bufPrintZ(&last_error, "{s}", .{@errorName(err)}) catch {};
}
var initialized = false;
var app_name = [_:0]u8{ 'h', 'e', 'r', 'd', 'r' };
var app_argv = [_][*:0]u8{&app_name};

const Args = struct {
    values: []const [*:0]const u8,
    index: usize = 0,
    pub fn next(self: *Args) ?[]const u8 {
        if (self.index == self.values.len) return null;
        defer self.index += 1;
        return std.mem.span(self.values[self.index]);
    }
};

fn initialize() !void {
    if (initialized) return;
    // Do not parse the host app's CLI or read a Ghostty configuration.
    try global.init(.{ .c = .{
        .renderer_only = true,
        .argc = app_argv.len,
        .argv = &app_argv,
        .environ = .{ .block = .{ .slice = std.c.environ[0..count: {
            var n: usize = 0;
            while (std.c.environ[n]) |_| : (n += 1) {}
            break :count n;
        } :null] } },
    } });
    initialized = true;
}

fn create(args: []const [*:0]const u8, points: f32, scale: f64) !*Renderer {
    try initialize();
    const self = try alloc.create(Renderer);
    errdefer alloc.destroy(self);
    var config = try Config.default(alloc);
    defer config.deinit();
    var it: Args = .{ .values = args };
    try config.loadIter(alloc, &it);
    try config.finalize();
    if (!config._diagnostics.empty()) {
        const diag = config._diagnostics.list.items[0];
        _ = std.fmt.bufPrintZ(&last_error, "{s}: {s}", .{ diag.key, diag.message }) catch {};
        return error.InvalidConfiguration;
    }
    const device = try renderer.Device.init(alloc, .{});
    errdefer device.deinit(alloc);
    var fonts = try font.SharedGridSet.init(alloc);
    errdefer fonts.deinit();
    var font_config = try font.SharedGridSet.DerivedConfig.init(alloc, &config);
    defer font_config.deinit();
    const grid_ref = try fonts.ref(&font_config, .{
        .points = points,
        .xdpi = @intFromFloat(@round(scale * 72)),
        .ydpi = @intFromFloat(@round(scale * 72)),
    });
    const grid = grid_ref[1];
    const view = herdr_metal_view_new() orelse return error.ViewUnavailable;
    errdefer herdr_metal_view_free(view);
    var metal = try renderer.Renderer.init(alloc, .{
        .device = device,
        .config = try .init(alloc, &config),
        .font_grid = grid,
        .size = .{
            .screen = .{ .width = grid.metrics.cell_width, .height = grid.metrics.cell_height },
            .cell = .{ .width = grid.metrics.cell_width, .height = grid.metrics.cell_height },
            .padding = .{},
        },
        .native_view = .{ .view = view, .scale = scale },
    });
    metal.external_render_state = true;
    self.* = .{ .device = device, .fonts = fonts, .view = view, .metal = metal, .scale = scale };
    return self;
}

pub export fn herdr_renderer_new(args: [*]const [*:0]const u8, argc: usize, points: f32, scale: f64) ?*Renderer {
    last_error[0] = 0;
    if (!std.math.isFinite(points) or points <= 0 or !std.math.isFinite(scale) or scale < 0.5 or scale > 8) {
        recordError(error.InvalidFontSizeOrScale);
        return null;
    }
    return create(args[0..argc], points, scale) catch |err| {
        recordError(err);
        return null;
    };
}

pub export fn herdr_renderer_free(self: ?*Renderer) void {
    const r = self orelse return;
    r.metal.threadExit(); // drains the GPU and releases swap-chain resources
    r.metal.deinit();
    r.fonts.deinit();
    r.device.deinit(alloc);
    herdr_metal_view_free(r.view);
    alloc.destroy(r);
}

pub export fn herdr_renderer_cell_size(self: *const Renderer) CellSize {
    return .{ .width = self.metal.grid_metrics.cell_width, .height = self.metal.grid_metrics.cell_height };
}

fn draw(self: *Renderer, term: c.terminal.Terminal, snapshot: c.render.RenderState, focused: bool, full: bool) !?*anyopaque {
    const t = term orelse return error.MissingTerminal;
    const s = snapshot orelse return error.MissingSnapshot;
    const cell = herdr_renderer_cell_size(self);
    const width = @as(u32, s.state.cols) * cell.width;
    const height = @as(u32, s.state.rows) * cell.height;
    if (width == 0 or height == 0) return error.EmptySnapshot;
    if (width > self.device.max_texture_size or height > self.device.max_texture_size) return error.SurfaceTooLarge;
    if (self.fresh or self.metal.size.screen.width != width or self.metal.size.screen.height != height) {
        herdr_metal_view_resize(self.view, width, height, self.scale);
        self.metal.setScreenSize(.{
            .screen = .{ .width = width, .height = height },
            .cell = .{ .width = cell.width, .height = cell.height },
            .padding = .{},
        });
    }
    if (self.metal.focused != focused) try self.metal.setFocus(focused);
    // Borrow the same snapshot Herdr prepared. Updating a second RenderState
    // would consume dirty flags and leave one renderer with stale rows.
    std.mem.swap(terminal.RenderState, &self.metal.terminal_state, &s.state);
    defer std.mem.swap(terminal.RenderState, &self.metal.terminal_state, &s.state);
    if (self.fresh or full) self.metal.markDirty();
    // The old renderer consumed this flag; new GPU resources must upload the
    // existing images again. Alternate-screen transitions already dirty it.
    if (self.fresh) t.terminal.screens.active.kitty_images.dirty = true;
    var mutex: std.Io.Mutex = .init;
    var state: renderer.State = .{ .mutex = &mutex, .terminal = t.terminal };
    try self.metal.updateFrame(&state, true);
    try self.metal.drawFrame(true);
    self.fresh = false;
    return herdr_metal_copy_frame(self.view, self.device.device.value, self.metal.api.queue.value, &last_error, last_error.len) orelse error.FrameCopyFailed;
}

pub export fn herdr_renderer_draw(self: *Renderer, term: c.terminal.Terminal, snapshot: c.render.RenderState, focused: bool, full: bool) ?*anyopaque {
    last_error[0] = 0;
    return draw(self, term, snapshot, focused, full) catch |err| {
        recordError(err);
        return null;
    };
}

// Internal regression-test seam: reach the upstream aging boundary without
// spending minutes submitting 100,000 GPU frames. Not used by the app.
pub export fn herdr_renderer_test_age(self: *Renderer, frames: usize) void {
    self.metal.terminal_state_frame_count = frames;
}
