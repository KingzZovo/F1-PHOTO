# server/

Rust 后端二进制 `f1photo`。

## 开发环境

- Rust stable (1.83+)
- PostgreSQL 16 + pgvector 0.5+

开发数据库（系统集群）：

```bash
sudo -u postgres psql -c "CREATE USER f1photo WITH PASSWORD 'f1photo_dev' CREATEDB;"
sudo -u postgres psql -c "CREATE DATABASE f1photo_dev OWNER f1photo;"
sudo -u postgres psql -d f1photo_dev -c "CREATE EXTENSION IF NOT EXISTS vector;"
```

复制环境变量模板：

```bash
cp .env.example .env
# 按需修改
```

## 常用命令

```bash
# 仅类型检查
cargo check

# 启动 (会自动跑 migrations)
cargo run

# 生产构建
cargo build --release
```

## 目录

```
server/
├── Cargo.toml
├── .env.example
├── migrations/                # sqlx-migrate, 编译期嵌入二进制
│   └── 20260426140000_init.sql
└── src/
    ├── main.rs               # 进程入口
    ├── lib.rs
    ├── config.rs             # 启动参数 + .env 加载
    ├── error.rs              # AppError + JSON 错误响应
    ├── logging.rs            # tracing 初始化
    ├── db.rs                 # PG 连接池 + migrate
    └── api/
        ├── mod.rs            # AppState + Router 顶层
        └── health.rs         # /healthz, /readyz
```

## 当前进度

M1 骨架：进程能起、迁移自动跑、`/healthz` `/readyz` 可用。后续提交补 auth、projects、master data、photos。
