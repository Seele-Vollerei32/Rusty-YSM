# SPM Cloud integration

同步后端已确定使用 SPM Cloud，不再移植 OpenYSM Minecraft 服务端网络包。

协议参考固定源码：

- spm-cloud: `552872ba2a3499cd17626eb37478de753bb38bbb`
- Sparkle-Morpher: `2e9dddadb09f74ae1456714fcfa118bc1d8c965e`

实现位于 `tools/spm_cloud.py`。对应上游 `CloudPlayerPresenceClient`、`CloudAssetClient`、`CloudAssetRef` 和服务端 `src/player.rs`：

- GET `/v1/instance`：校验 `protocol == spm.cloud.v1`。
- POST `/v1/players/appearances/query`：最多 64 个 UUID，检查返回集合、重复/未请求 UUID，支持 include_offline。
- PUT `/v1/players/me/appearance`：已绑定 identity_id、实体 UUID、expected_revision 和精确模型引用；selection=None 清除发布。服务端负责身份授权与版本冲突，客户端不盲重试。
- GET `/v1/assets/{id}/revisions/{revision}/content`：认证下载，拒绝部分响应，限制大小，校验 SHA-256 后原子保存。每次调用重新授权，不离线重放缓存，不将 token 转发到重定向地址。
- 缓存按 Cloud origin 隔离。ZIP 展开限制路径、文件数和总尺寸，拒绝符号链接与重复路径。

## 启动时读取 Cloud 外观

先通过 SPM 的账号/身份流程获得有效 access token；本实现尚未接入自动登录及游戏身份绑定 UI。token 从环境读取，不作为命令行参数或日志输出。

```powershell
$env:SPM_ACCESS_TOKEN = '<access token>'
python tools/cloud_launch.py --origin https://your-cloud.example --player <Minecraft实体UUID> --resources C:\prepared-resources
```

该命令查询指定玩家当前外观，下载精确 ZIP 版本，解析其中的 `ysm.json` 后启动现有 Horizon 渲染原型。临时解包目录在客户端退出后删除。`--dry-run` 仍会查询与下载，只是不启动客户端。

## 多玩家轮询模式

```powershell
$env:SPM_ACCESS_TOKEN = '<access token>'
python tools/cloud_play.py --origin https://your-cloud.example --resources C:\prepared-resources
```

可通过 `--client <客户端路径>` 选择已有客户端；启动前检查客户端文件和资源目录。退出时先请求子进程终止，超时后强制结束并回收，避免后台进程无限等待。

`cloud_play.py` 启动独立 Python 工作进程和 Horizon，凭据仅传给工作进程。Horizon 发布当前网络实体 ID→UUID 映射，工作进程每轮查询 SPM 外观并下载验证资源；每轮完成后间隔约 3 秒。客户端使用独立模型实例与动态图集槽，模型按实体 ID 应用，不再把 Cloud 模型用于所有玩家。

当前上限为 16 名玩家，每张 PNG 最大 512×512（超限回退原版）。玩家集合代次不匹配的响应被丢弃；状态有效期 15 秒，失败/撤销/过期回退原模型。稳定实体排序避免槽位反复交换。离开网络世界后无新刷新，旧授权最多在有效期结束时失效。客户端退出时终止工作进程并清理临时资源。

这是已接入客户端并通过编译的轮询实现。选中的 `motion.animation_key` 会解析为包内动画 clip；`motion.roaming` 会作为 Molang 的 `variable.<name>` 查询，`started_at_unix_ms` 用于动作时间基准。未知动作 key 会安全回退到中立姿态。`display_state.state` 已透传到原生模型，并用于 Molang 查询：health、max_health、experience_level、food_level、is_flying、is_blocking、strafe_input、vertical_input、forward_input；缺失值回退原有查询逻辑。它仅影响动画输入，不修改游戏实体属性。动态 motion/display_state 每轮更新，资源缓存键只包含模型版本、纹理与动画片段；同一片段的开始时间、变量及显示状态变化不会重新解析几何或上传纹理，撤销值也会即时覆盖旧输入。effect_amplifiers 和客户端世界上下文匹配尚未接入。真实多人画面仍需实际 Cloud 实例联调。

缓存命中时校验本地 SHA-256，再发带 `If-None-Match` 的条件 GET；仅接受匹配 ETag 的 200/304，拒绝授权时删除缓存。失败采用最多 30 秒的指数退避；请求文件超过 5 秒未更新时停止刷新旧玩家。

可选设置 `SPM_REFRESH_TOKEN`：发现、查询或资源请求返回 401 时刷新一次会话并重试。轮换后的令牌仅存在工作进程内存中，不落盘；退出后不能假定原环境中的 refresh token 仍有效，重启应提供新的会话凭据。

PNG 解码已移到后台进程，客户端校验 RGBA 尺寸后上传。当前解码支持 8-bit RGB/RGBA、灰度/灰度加透明度和调色板非交错 PNG，以及 RGB/灰度透明色键和调色板透明度；包含 CRC、调色板索引、透明度长度、头部顺序和解压大小校验。同时支持 1/2/4-bit 灰度和调色板打包像素，过滤按字节处理，行末填充位不作为像素。16-bit RGB/RGBA、灰度和灰度加透明度也已支持：先按完整 16 位样本比较透明色键，再取高字节转换到 8-bit GPU 纹理。Adam7 交错尚不支持。不同纹理按像素哈希分开保存。

仍需实现：WSS 低延迟动作通道、登录/身份绑定、上传与模型库 UI，以及更多 PNG 格式。当前动作使用 HTTP 轮询，首次 JSON 几何和动画解析仍位于游戏线程。`/openysm list` 可检查本地模型状态，`/openysm cloud off` 清理当前 Cloud 模型并持续暂停请求发布；`/openysm cloud on` 重新启用。切换时递增请求代次，使关闭前的响应失效。ZIP 以外的 ysm/bbmodel/gltf/glb 格式当前显式拒绝。Cloud 手臂、动画控制器、motion expressions 和 display_state.effect_amplifiers 尚未映射。

验证：Python unittest discovery 共 29 项测试通过（含损坏请求拒绝及后续有效请求恢复）（含请求在下载期间撤销、撤销后无新下载）（新增 16-bit 四种颜色类型、双字节 Sub 过滤和低字节不同的透明色键比较）（含低位深跨字节像素、行末填充位、Sub 过滤、灰度、调色板和透明色键像素结果及非法输入检查），覆盖显示状态透传、撤销、未知值保留、整数/布尔类型区分、非有限数值、数值边界、维度标识和效果等级校验；Cloud 开关改动已通过真实 apply 和完整客户端离线 cargo check（36.71s）；先前的 Windows E_IO 未复现，未修改目录权限，根因尚未确定。此前显示状态改动通过真实 Horizon plan/apply 和完整客户端离线 cargo check（38.82s）。此前完整客户端优化构建通过（2m26s），覆盖条件 GET/304、缓存授权、轮换刷新、过期撤销、逐玩家资源、RGBA 隔离和 PNG 解码边界；HTTP 使用本地协议夹具，其余网络边界使用替身；逐玩家 Hook 通过真实 Horizon plan/apply 和完整客户端 cargo check；没有提供 Cloud 实例及账号，因此尚未进行真实服务联调。未发送发布、上传或清除操作到任何外部实例。
