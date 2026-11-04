# Herdr

[English](README.md) · 简体中文

Herdr 是一款面向终端与编程 Agent 的桌面应用。把工作按空间（Space）归类管理，一眼看清哪些 Agent 需要你处理，关闭窗口后终端依然在后台继续运行。

## 安装

### Linux 与 macOS

```sh
curl -fsSL https://releases.herdr.dev/app/install.sh | sh
```

Linux 需要桌面会话和可用的 Vulkan 驱动。macOS 提供 Apple Silicon 版本；Intel Mac 用户可以[从源码构建](#从源码构建)。Linux 提供 x86-64 和 ARM64 版本。

安装脚本会把 Herdr 添加到应用程序列表，并将命令行工具放到 `~/.local/bin`。
如果安装脚本提示，请将该目录加入你的 `PATH`。
在 macOS 上，应用位于 `~/Applications/Herdr.app`。
如果希望在 Apple Silicon 上拖拽安装，可以下载 [Herdr.dmg](https://releases.herdr.dev/app/Herdr.dmg) 并将 Herdr 拖入 Applications。

在 macOS（Apple Silicon）上，也可以使用 [Homebrew](https://brew.sh/) 安装：

```sh
brew install --cask herdr-app
```

### Windows

在已安装 `curl` 和 `tar` 的 x64 Windows（Windows 10 1803 及以上版本）中打开 PowerShell 运行：

```powershell
irm https://releases.herdr.dev/app/install.ps1 | iex
```

Herdr 会出现在开始菜单中。安装脚本会将 `%LOCALAPPDATA%\Programs\herdr-app` 添加到当前用户的 `PATH`，新开一个终端即可生效。

Shell 和 PowerShell 安装脚本会下载并执行代码。如果你希望先审阅脚本内容，可以先阅读 [shell 安装脚本](distribution/install.sh) 或 [PowerShell 安装脚本](distribution/install.ps1)。
如果系统中尚未安装 [herdr-core](https://github.com/herdrdev/herdr-core)，安装脚本也会一并安装。
当应用发现当前 core 不可用时，也会协助安装或更新。

## 组织你的工作

一个**空间（Space）**对应一台机器上的一项工作（通常是一个代码仓库）。每个空间包含多个**标签页（Tab）**。将标签页拆分为多个**窗格（Pane）**，即可将多个终端或 Mod 面板并排展示。

未命名的空间会自动跟随其第一个终端所在的文件夹名称。如果你希望名称固定不变，可以重命名空间或标签页。
新建标签页默认在当前聚焦终端所在的文件夹中启动。

Agent 侧边栏会将各个空间里的 Agent 汇总到同一个列表中，包括由其他 Agent 启动的子任务助手（Worker）。
支持上报状态的 Agent 会显示为以下状态：

- **工作中（Working）：** 正在忙碌运行。
- **需要你 / 被阻塞（Blocked）：** 正在等待你的输入或确认。
- **已完成（Done）：** 在你查看其他内容期间完成了工作。
- **空闲（Idle）：** 当前未在执行任务，或你已经查看过的已完成 Agent。

点击任意 Agent 即可跳转到它所在的视图。没有独立窗格的子任务助手会在当前标签页上方以**预览（Peek）**浮层打开；如果你想把它留在手边与其他工作并排查看，可以点击保留为标签页。
在终端文本中按住 Ctrl 点击活动终端地址（macOS 上为 Cmd 点击）即可预览该终端。
像 `t7` 这样的地址代表与当前终端位于同一台机器上的终端；`vps/t7` 则代表另一台机器上的终端。

窗口负责编排与展示终端，而终端实际运行在 `herdr-core` 中。
关闭窗口不会中断终端运行；只有关闭窗格或标签页才会结束对应的终端。
当电脑重启或 core 异常退出后，保留的窗格会在上次所在的文件夹中重新打开新的 Shell，但不会自动重新运行之前的程序。

## 在其他机器上工作

在“设置 → 机器”（Settings → Machines）或侧边栏底部的“添加机器”（Add machine）中添加 SSH 远程机器。
你可以直接从 `~/.ssh/config` 中选择主机，或手动输入 SSH 连接方式。
建在该机器上的空间会直接在远程机器上运行终端，而不是在本机运行。

每台机器都在各自的 core 中保存自己的空间、标签页和分屏结构。连接到同一台机器的多个屏幕会共享这套结构与实时终端，但各自保持独立的焦点和缩放比例。
只要远程机器及其 core 保持运行，即使你关闭本地窗口或合上笔记本休眠，远程机器上的工作依然会继续运行。

## 打造你自己的界面

打开命令面板可以按名称搜索并执行任何操作；打开“设置”（Settings）可以修改界面语言（支持简体中文与英文）、字体、配色、提示音及其他偏好设置。
内置布局包括 Herdr、Terminals 和 Herdr 98。
**布局（Layout）**决定窗口的整体排布，而 **Mod** 则是绘制各个面板、菜单和对话框的小型 JavaScript 程序。

如果你想修改内置布局或 Mod 的结构与代码，请先复制（Duplicate）一份。你复制出来的版本完全归你所有；Herdr 每次启动时只会更新内置版本。
保存你自己的布局、Mod 或快捷键文件后，窗口会自动热重载。
如果编辑出现语法或运行错误，窗口会保留上一次正常工作的版本并提示错误详情。

按 Ctrl+Shift+P（macOS 上为 Cmd+Shift+P）可查看所有命令及其快捷键。你可以在 `keys.json` 中自定义任何快捷键；具体格式（包括具名、带类型的 JSON 命令参数）请参阅 Mod 指南中的[快捷键章节](defaults/AGENTS.md#keys-keysjson)。

[布局与 Mod](docs/mods.md) · [命令参考](docs/commands.md)

## 更新与隐私

通过安装包安装的发行版会在后台自动检查更新，并将新版本下载到当前运行版本旁边。当你退出并重新打开应用，或手动选择更新操作时，新版本就会生效；在此期间，你的终端会在 `herdr-core` 中保持运行。
“设置 → 更新”（Settings → Updates）会显示当前可用的更新。每次更新后会打开“新功能”（What's New）页面，你也可以随时从命令面板再次打开它。
从源码编译的版本不会自动更新。

正式发行版默认开启匿名使用数据统计，你可以在“设置 → 隐私”（Settings → Privacy）中关闭。
Mod 是受信任的代码而非沙箱扩展。在安装他人编写的 Mod 之前，请先阅读[安全与隐私说明](docs/security.md)。

## 面向 Agent 的命令

```sh
herdr-app --skill
```

该命令会输出供 Agent 操控窗口的指南（`herdr-core` 的 skill 也会将 Agent 引导至此）。
使用 `herdr-core` 来运行和协调后台程序；使用 `herdr-app` 来调整界面上显示的内容。

```sh
herdr-app ls
herdr-app status
herdr-app commands
```

前两个命令用于检查当前打开的窗口；`herdr-app commands` 则无需打开窗口即可列出所有可用命令。
各命令的参数说明请参阅[命令参考](docs/commands.md)。

## 从源码构建

如需反馈 Bug、提出 Mod API 需求或贡献代码，请参阅[贡献指南](CONTRIBUTING.md)。

请连同子模块一起克隆本仓库，并使用与 [CI](.github/workflows/ci.yml) 一致的 Rust 1.95.0 和 Zig 0.16.0。
在 Linux 上构建需要安装该 CI 工作流中列出的开发依赖包。

```sh
git clone --recurse-submodules https://github.com/herdrdev/herdr-app.git
cd herdr-app
just build
just test
just install
```

请先安装 [just](https://github.com/casey/just) 以运行上述命令。
平台安装命令会编译 release 版本并将应用添加到桌面环境。
在 Linux 上，安装会软链接到当前源码检出目录，因此请勿移动或删除该目录。
退出并重新打开 Herdr 即可使用新编译的版本。

文档检查还需要安装 [Bun](https://bun.sh)：`just docs-check` 用于检查生成的文档页面与 CLI 示例；`just docs-generate` 用于重新生成参考文档。

如需在 Windows、Linux 或 macOS 上对比渲染改动前后的性能，可以针对不同构建运行相同的固定基准负载：

```sh
python scripts/bench.py --bin <app executable> --set spinners --seconds 20
```

基准测试脚本要求 `hc` 已在 `PATH` 中，并使用当前运行脚本的 Python 解释器。它会输出一个包含运行配置、计数器和结果的独立产物目录，随后自动停止其私有的应用实例、终端和 core。两组测试均使用 Python 测试夹具程序；默认测试集中的 TUI 负载替代了早期的 `btop` 测试，`--only spinners` 则仅测量轻量原生动画场景。测试期间请保持窗口可见且显示器常亮，并在相同的窗口活动状态和显示器设置下对比；非活动窗口可能会以较低帧率绘制。Windows 会在每个测量边界记录窗口可见性、前台状态和显示器刷新率（Hz），其他平台则将这些观测项标记为不可用。可见性并不等同于未被遮挡。绘制次数（Draws）指窗口渲染调用次数；Render-to-paint 耗时在场景定稿、后端绘制、上传和上屏前停止统计，且独立于操作系统进程 CPU 时间。这两项指标均不直接测量物理屏幕呈现帧率或 FPS HUD 推算的 MAX FPS。

在 macOS 上进行对比测试时，可运行 `python scripts/bench_compare.py ghostty --root <artifact folder>` 复用已生成的模拟 Agent。运行基准测试自检：`python -m unittest discover -s scripts -p test_bench.py`。
