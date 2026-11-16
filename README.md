# OpenYSM on Horizon

这是基于 `horizon-native-mod-template` 的原生移植起点，已有真实 Horizon 玩家绘制接入和从 `openysm.cpp` 移植的 C++ 加速运行库。当前版本为 **0.1.0 原型**，尚未实现完整 OpenYSM 功能兼容。

固定 Horizon `ba6344a59cb9e755b730dcf02285fc0d33b97d3c`、horizon-mixins `d9c83d7096b7ea5b83678da2efdfd6161f74997e`，沿用模板的源码 Mixin 路线。产物是自定义 Horizon 客户端及 `.hnmod` 源码包。

## 已实现

- 原生 bundle crate，由 horizon-mixins 注入依赖，Rust 和 C++ 一起编译，无 JVM/JNI 依赖。
- 从 OpenYSM.cpp 复用 SIMD 矩阵、GPU 网格烘焙和骨骼层次计算，添加有长度检查的 C ABI 与 Rust 所有权封装。x86 使用 SSE，ARM64 使用 sse2neon，不强制要求 AVX2/FMA。
- Bedrock `minecraft:geometry` 首个几何体：骨骼层次、骨骼/立方体 pivot 和 rotation、inflate、box UV、每面 UV、mirror。
- 动画：旋转、位移、缩放、线性/Catmull-Rom 关键帧、pre/post 跳变、时长推断和循环。
- Molang 子集：算术、比较、逻辑、三元表达式及 sin/cos/abs/sqrt/floor/ceil/round/min/max/pow/clamp/lerp；查询支持 anim_time、life_time、ground_speed、is_moving、is_alive。尚未接入的查询返回 0，赋值和未知函数报错。
- `OPENYSM_TEXTURE_FILE` 加载 PNG 并注册到 Horizon 图集，尺寸上限 4096×4096、文件上限 32 MiB；首次构建图集时加载，尚无纹理热更新。
- 拦截 Horizon 玩家实体的 body 绘制，保留 Horizon 的护甲及其他 entity layers。模型未配置、加载失败或纹理不存在时回退原模型。
- CPU SIMD 蒙皮输出 Horizon 实体顶点，由现有 wgpu 管线绘制。C++ 保留原始 32 字节 GPU 顶点和 144 字节骨骼布局。

## 构建

需要 Rust 1.95.0、Python 3.12+ 和 C++17 编译器。Windows 已验证 MSVC；Linux/ARM64 尚未验证。上游 checkout 放在此目录旁边，或者通过参数指定；脚本导出固定 Git 提交，修改只发生在 `target/`。

```sh
cargo fetch --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
python tools/horizon.py stage --horizon ../horizon --mixins ../horizon-mixins
python tools/horizon.py check
python tools/horizon.py build
python tools/package.py
```

`tools/horizon.py` 要求 horizon-mixins checkout 位于固定提交且引擎源码干净。首次运行会获取构建依赖。`check/build/test` 使用 staged workspace，不会修改原 Horizon checkout。`test` 只编译客户端测试；运行库测试用上面的 `cargo test`。

Windows 的可执行文件在 `target/horizon-cargo/fast/horizon-client.exe`，Linux 对应无 `.exe` 后缀。资源需要使用固定 Horizon 版本准备。

## SPM Cloud 多玩家模型

```powershell
$env:SPM_ACCESS_TOKEN = '<access token>'
python tools/cloud_play.py --origin https://your-cloud.example --resources C:\prepared-resources
```

此模式启动后台同步进程，按当前网络玩家 UUID 查询 Cloud，并为各玩家分别加载模型和纹理。当前支持 16 名玩家、512×512 PNG 和 ZIP 内 YSM 模型包；失败或授权过期回退原版。HTTP 动作 clip 和 Molang roaming 查询已接入；WSS 低延迟通道、display_state、登录绑定及真实服务器联调仍需外部 Cloud 实例。需要本地 Python 工具配合，单独安装 `.hnmod` 不会启动同步进程。详见 [SPM_CLOUD.md](SPM_CLOUD.md)。

## 模型包启动与运行时控制

可以直接选择解包后的 OpenYSM 模型包，无需逐项填写环境变量：

```sh
python tools/launch.py --pack ../OpenYSM/common/src/main/resources/assets/yes_steve_model/builtin/default --resources /path/to/prepared-resources
# 可选 --texture blue、--clip <动画完整名称>；--dry-run 只显示配置。
```

工具解析 `ysm.json` 中的主模型、纹理列表及默认纹理，限制资源路径必须位于模型包内。不会自动播放完整 OpenYSM 状态机。

本地命令处理路径提供 `/openysm off`、`/openysm reload`、`/openysm model <geometry路径>`。后者仅切换几何，继续使用当前图集纹理并清除旧动画；加载失败保留旧模型。消息目前写入终端。多人服务器聊天路径尚未验证，不声明支持服务器侧命令。

`OPENYSM_LEFT_ARM` / `OPENYSM_RIGHT_ARM` 可指定独立手臂几何，使用当前模型纹理和 Horizon 手部变换。几何需要围绕手部原点制作；尚未自动解释原版 arm.json 的左右骨骼分组，也未接入手臂动画。未配置时保留原手臂。

## 选择模型

模型在首次绘制时读取一次，修改后重启客户端。当前对所有玩家使用同一配置。

PowerShell 示例：

```powershell
$env:OPENYSM_MODEL = (Resolve-Path ../OpenYSM/common/src/main/resources/assets/yes_steve_model/builtin/default/models/main.json).Path
$env:OPENYSM_TEXTURE = 'entity/player/wide/steve'
$env:OPENYSM_TEXTURE_FILE = (Resolve-Path ../OpenYSM/common/src/main/resources/assets/yes_steve_model/builtin/default/textures/default.png).Path
Remove-Item Env:OPENYSM_TEXTURE -ErrorAction SilentlyContinue
# 可选：数值或受支持 Molang 子集的动画文件。
$env:OPENYSM_ANIMATION = 'C:\models\numeric.animation.json'
$env:OPENYSM_CLIP = 'animation.model.idle'
./target/horizon-cargo/fast/horizon-client.exe --resources C:\prepared-resources
```

`OPENYSM_TEXTURE` 是 **Horizon 已加载的图集 sprite key**，不是 PNG 文件路径。默认 Steve 纹理只适合验证绘制链；自定义模型正确显示需要将其纹理纳入资源图集并使用对应 key。指定 `OPENYSM_TEXTURE_FILE` 并不设置 `OPENYSM_TEXTURE` 时，会自动选择新注册的 `openysm/model` sprite。PNG 会随 Horizon 图集上传到 GPU，修改 PNG 后需要重启。

直接验证真实模型解析和 C++ 蒙皮：

```sh
cargo run --example validate_model -- ../OpenYSM/common/src/main/resources/assets/yes_steve_model/builtin/default/models/main.json
```

该默认模型验证结果：64 个骨骼，4128 个顶点，输出坐标和法线均为有限数值。模型资源不包含在本项目源码包中。

## 尚未迁移

- OpenGL GPU skinning/compute shader 尚未转换为 wgpu/WGSL；现有 GPU 缓冲布局不代表已经启用 GPU 蒙皮。
- 完整 Molang（变量、赋值、自定义查询）、动画状态机/控制器、Bezier、音效/粒子时间线、每玩家模型与动作选择。
- `.ysm` 容器、压缩/加密、完整多人实时同步/上传。同步后端改为 **SPM Cloud**，不实现 OpenYSM 原有服务器协议；已有启动时导入及逐玩家轮询、验证下载与动态纹理上传，参见 [SPM_CLOUD.md](SPM_CLOUD.md)。
- 模型管理界面、轮盘、模型预览选模、完整第一人称手臂动画/骨骼分组及各 Forge/Fabric 兼容模块。
- 运行时纹理热更新、材质/透明排序和 OpenYSM 专用挂点。仍使用 Horizon 自己的护甲、披风及物品挂点。
- 纹理/手臂热重载、配置文件及多人资源生命周期。主模型已有显式 reload/model 命令。

当前只经过编译和自动化测试，没有用准备好的游戏资源进行交互渲染验证。不要将该版本视为 OpenYSM 的完整可玩替代品。

## 上游差异与授权

`openysm.cpp/dllmain.cpp` 的 CPU JNI 签名没有当前 Java 的 `stateArray`，CPU Quad 输入也没有 Java 的透明字节；Java GPU serializer 则仍是每 Quad 92 字节。本移植直接从几何构建 GPU 格式网格，再用同一骨骼结果执行 CPU SIMD 蒙皮，避免混用两种序列化布局；不加载原有 JNI DLL。

模板工具和许可证来自 `horizon-native-mod-template`（MIT）；C++ 派生代码及 `native/LICENSE` 来自 OpenYSM.cpp（MIT），sse2neon 保留文件内许可证。原始 OpenYSM 和模型资源保持在工作区原目录，未修改或打包。详见 [NOTICE.md](NOTICE.md)。
