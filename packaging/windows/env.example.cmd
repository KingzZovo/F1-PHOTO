@REM Place at %ProgramFiles%\F1Photo\env.cmd. NSSM imports these via
@REM AppEnvironmentExtra in install.cmd; this file is for ad-hoc runs.
set F1P_BIND=0.0.0.0:8080
set F1P_JWT_SECRET=change-me-32-character-secret-rotate-on-incident
set F1P_USE_BUNDLED_PG=1
set F1P_BUNDLED_PG_DIR=%ProgramFiles%\F1Photo\bundled-pg\bin
set F1P_BUNDLED_PG_DATA=%ProgramFiles%\F1Photo\bundled-pg-data
set F1P_BUNDLED_PG_PORT=5544
set F1P_BUNDLED_PG_PASSWORD=please-rotate-me
set F1P_DATA_DIR=%ProgramFiles%\F1Photo\data
set F1P_MODELS_DIR=%ProgramFiles%\F1Photo\models
set F1P_MAX_UPLOAD_MB=20
set F1P_INFERENCE_THREADS=4
set ORT_DYLIB_PATH=%ProgramFiles%\F1Photo\runtime\onnxruntime.dll
