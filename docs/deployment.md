# 部署指南

> 目标：一键脚本、纯离线、同时支持 Linux 与 Windows。

## 1. 发布产物

每次发布运行 `make release`，产出：

```
release/
├── f1-photo-{ver}-linux-x86_64.tar.gz
│   ├── bin/f1-photo                  # Rust 单二进制（嵌入 Web 静态）
│   ├── lib/libonnxruntime.so.*
│   ├── models/                       # InsightFace + YOLOv8n + DINOv2 ONNX
│   ├── postgres/                     # 便携 PostgreSQL 16 + pgvector
│   ├── install.sh
│   ├── systemd/f1-photo.service
│   ├── systemd/f1-photo-pg.service
│   └── README.txt
├── f1-photo-{ver}-windows-x86_64.zip
│   ├── bin\f1-photo.exe
│   ├── lib\onnxruntime.dll
│   ├── models\
│   ├── postgres\
│   ├── install.ps1
│   ├── nssm\nssm.exe
│   └── README.txt
└── f1-photo-android-{ver}.apk
```

## 2. Linux 安装脚本 `install.sh`

### 交互项

- 安装路径（默认 `/opt/f1-photo`）
- HTTP 监听端口（默认 `8080`）
- PostgreSQL 端口（默认 `5544`）
- 管理员账号 / 密码
- 数据目录（默认 `/var/lib/f1-photo`）

### 步骤

1. 检查依赖（`tar`, `bash`, `systemctl`, `useradd`）。
2. `useradd --system --no-create-home --shell /usr/sbin/nologin f1photo`。
3. 解压产物到 `$INSTALL_DIR`。
4. `chown -R f1photo:f1photo $DATA_DIR $INSTALL_DIR`。
5. 初始化便携 Postgres：
   - `postgres/bin/initdb -D $DATA_DIR/pg --username=f1photo --auth-local=trust`
   - 启动实例并 `CREATE DATABASE f1photo;` `CREATE EXTENSION vector;`
   - 生成随机 DB 密码写入 `$INSTALL_DIR/etc/f1-photo.env`。
6. 写入 systemd unit。
7. 记录默认 settings + 创建管理员账号（调 `f1-photo bootstrap --admin <user> --password <pass>`）。
8. 启动服务并 `curl -fsS http://127.0.0.1:$PORT/healthz`。
9. 打印总结：
   ```
   ============================================================
   F1-Photo 部署完成
   HTTP：       http://<host>:8080
   PostgreSQL： 127.0.0.1:5544 (仅本机)
   数据目录： /var/lib/f1-photo
   日志：     /var/log/f1-photo
   头个账号： admin / ******
   ============================================================
   ```

### `f1-photo.service`（示意）

```
[Unit]
Description=F1-Photo Server
After=network-online.target f1-photo-pg.service
Requires=f1-photo-pg.service

[Service]
User=f1photo
Group=f1photo
EnvironmentFile=/opt/f1-photo/etc/f1-photo.env
ExecStart=/opt/f1-photo/bin/f1-photo serve
Restart=on-failure
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

## 3. Windows 安装脚本 `install.ps1`

### 交互项

与 Linux 一致：HTTP 端口、PG 端口、admin 账号、数据目录（默认 `C:\ProgramData\F1-Photo\data`）、安装目录（默认 `C:\Program Files\F1-Photo`）。

### 步骤

1. 以管理员权限运行 PowerShell。
2. 解压产物。
3. 初始化便携 Postgres（`postgres\bin\initdb.exe`） → `pg_ctl start` → `psql` 创库 + `CREATE EXTENSION vector;`
4. 用 `nssm install F1-Photo-Pg` 与 `nssm install F1-Photo` 创建两个服务。
5. 设置 `Start=Automatic`，依赖关系。
6. 启动后 `Invoke-WebRequest http://127.0.0.1:$Port/healthz`。
7. 输出总结文本（同 Linux）。

## 4. 升级

```
f1-photo upgrade --pkg f1-photo-{newver}.tar.gz
```

- 市升级脚本黑名单安装目录，覆盖 `bin/`、`models/`、`web/`。
- 保留 `etc/`、`data/`、`postgres/data/`。
- 运行 `f1-photo migrate` 进行 sqlx 迁移。
- `systemctl restart f1-photo.service`。
- 升级失败 → 自动回滚上一个 `bin.bak`。

## 5. 反向代理与 HTTPS

- 服务默认启动 HTTP。
- 推荐用户自行在外层部署 nginx/Caddy + 自签证书。
- `f1-photo.env` 允许 `BIND=127.0.0.1:8080` 限制仅内网。

## 6. 备份与恢复

- `f1-photo backup --out /backup/f1-photo-YYYYMMDD.tar.zst`
  - 包含 `pg_dump` 、`data/orig/` 、`data/archive/` 、`models/`。
- `f1-photo restore --in <pkg>` 复原（需服务停止）。
- 定期作业由系统 cron / Task Scheduler 调用。

## 7. 项目验收

冷装验收清单（干净 VM）：

1. Ubuntu 22.04 + Windows 11 各一台。
2. 复制发布包 → 走 install.sh / install.ps1 → 接收默认参数。
3. 访问 `/healthz` `/readyz`。
4. 创建一个工单 + 上传 5 张人脸照 + 5 张工具照。
5. 检查识别条目页 + 归档路径 + zip 打包下载。
6. 设备安装 APK，拍照上传 + 版本检查。
7. 重启服务、重启机器 → 服务自动拉起。

验收通过后才可发布。

## 8. 不做的事

- 不提供 Docker 镜像（可在后期补，但单机场景不是优先项）。
- 不集成云部署组件。
- 不在生产环境默认启用 OpenTelemetry collector，需要时在后台手动启用。
