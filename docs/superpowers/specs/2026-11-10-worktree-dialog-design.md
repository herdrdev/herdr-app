# 合并 worktree 对话框

## 意图

`+` 菜单和 space 右键里的 New worktree、Open worktree 收成一个对话框. 同一个地方可以: 从当前 HEAD 新建分支, 检出已有本地分支, 检出已有远程分支, 打开已有 worktree. 分支已经在某个 worktree 里时, 点它打开那个 space.

## 目标

- 两个菜单各只留一项, 打开同一个对话框.
- 四个来源: New, Local, Remote, Open. 从菜单打开时默认 Remote.
- 检出的是那个分支本身. 已经检出则打开它所在的 space; 还没有 space 时, 在那个 checkout 上新建一个.
- 新建分支仍在这个对话框的 New 里, 从当前 HEAD 起.
- 路径仍是 `~/.herdr/worktrees/<repo>/<branch>`, space 不另起名字.
- Remove worktree 保持独立.
- 外观沿用现在的对话框: 来源切换、输入框、列表、右下角键盘提示.

## 非目标

- 自定义 worktree 路径, 或在对话框里给 space 起名.
- 远程分支检出时另起一个不同的本地分支名.
- 用 `--force` 把已经检出的分支再检出一次.
- 改 Remove 的流程.

## 不做的事

- 一个已经检出的分支再得到第二个 worktree.
- 在 Local 或 Remote 里选中已有分支时, 从当前 HEAD 另开一个新分支.

## 方案

1. **四个来源.** 每个来源一块屏幕. New 只有分支名输入框. 另外三个是可过滤的列表. 采用这个. 菜单打开时默认 Remote.
2. **三个来源.** 新建分支放在 Local 的输入框里, 名字不存在就从 HEAD 创建. 没采用: New 要单独占一个来源.
3. **一个输入框.** 同时过滤本地、远程和已有 worktree, 没有匹配就创建. 没采用: `feature` 和 `origin/feature` 会同时出现.

## 行为

对话框标题仍是 `New worktree of <repo>`. 来源四个字: New, Local, Remote, Open. 点字切换. 列表里上下键移动, 回车执行, Esc 关闭. 左右键留给输入框.

从 `+` 菜单和 space 右键进入时 `action` 为 `"remote"`. 旧调用仍可用: `"new"` 打开 New, `"open"` 打开 Open, `"local"` 打开 Local, `"remove"` 仍是删除确认. 带 `{branch, tab?, argv?}` 且不停留在对话框上的立即创建, 按下面的解析走, 成功后 `space.new` 带上 `tab` 和 `argv`.

路径: `~/.herdr/worktrees/<repo>/<slug>`. `slug` 沿用现在的规则.

**New.** 只创建. `git worktree add -b <name> <path> HEAD`. 本地已经有这个名字时停下来, 说明分支已存在, 不检出. 空名字仍提示要输入分支名.

**Local.** 列出 `refs/heads`. 该分支已经在某个 worktree 里 (含主 checkout): 打开那个 space, 没有 space 就在那个目录上新建. 否则 `git worktree add <path> <branch>`.

**Remote.** 列出 `refs/remotes`, 不包含 `HEAD`. 行上写 `origin/feature/name` 这种全名. 本地分支名是远程名之后的部分 (`feature/name`). 同名本地分支已存在: 跟 Local 一样, 检出它或打开它的 space, 不改它指向哪一个远程. 同名本地分支不存在: `git worktree add --track -b <short> <path> <remote>/<short>`.

**Open.** 就是现在的 checkout 列表: 过滤, 已有 space 标 open, 回车或点击打开; 没有 space 则在该目录上新建.

**立即创建 `{branch}`.** 先按 Local 的规则找这个本地分支. 本地没有时, 若恰好一个远程有这个短名, 按 Remote 的规则跟踪它. 否则按 New 的规则从 HEAD 创建. 已经检出时打开 space, 不再让 `git worktree add` 失败.

四个来源 (除 Remove 外) 的对话框都是 560×480. Remove 维持现在的小尺寸. 本地分支、远程分支、worktree 列表在打开时一起读, 切换来源不再等待.

## 我定的

- 菜单文案留 `New worktree…`. 对话框标题不改.
- New 发现重名只报错, 不改去检出. 检出在 Local 和 Remote.
- 来源用点击切换, 不加左右键切换.
- `action: "new"` 和 `action: "open"` 仍打开对应来源, 菜单自己传 `"remote"`, 这样默认是 Remote, 旧调用还在.
- 远程行的本地名取第一个 `/` 之后的部分. 远程名本身不含 `/`.
- 同名本地分支已经存在时, 点哪个远程行都使用这个本地分支.
- 列表上的 open 标记只表示已经有 space. 没有 space 但分支已经检出时, 点击仍打开 (并建 space).
- 提示语: New 说明新分支从这里开始. Local 和 Remote 说明已经检出的会打开那个 space, 否则检出到 `~/.herdr/worktrees/<repo>/`. 右下角提示跟着当前行: 创建、检出或打开.

## 改动面

- `defaults/mods/worktree/main.js`: 四个来源和上面的 git 命令.
- `defaults/mods/menu/main.js`, `defaults/mods/new-menu/main.js`: 收成一项, 尺寸 560×480, `action: "remote"`.
- `defaults/AGENTS.md`: 同一改动里改掉 New worktree 与 Open worktree 两条入口的说明.
- `scripts/worktree.test.ts`: 覆盖四个来源的命令、已检出改为打开 space、New 重名报错、`{branch}` 的三条解析.

不改 Rust, 不改 Remove.
