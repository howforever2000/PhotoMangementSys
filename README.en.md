# PhotoManagementSys — Local Photo Management System

> A local-first desktop photo management application with a built-in AI visual recognition microservice. Photos are automatically classified, faces, scenes and text are recognized — all locally, fully offline-capable.

[![License](https://img.shields.io/badge/license-Private-1f6feb.svg)]()
[![Platform](https://img.shields.io/badge/platform-Windows%20(x64)-1f6feb.svg)]()

---

## 1. Positioning

PhotoManagementSys is a desktop application designed for personal photo libraries. Its core positioning is as follows:

- **Local storage**: Photos and all management data reside on the local machine and are never uploaded to any server.
- **Offline AI**: A built-in Visual Content Recognition (VCR) microservice performs classification, detection, face recognition, and OCR inference locally via ONNX Runtime.
- **Folder abstraction**: An album is an abstraction of a local folder. Existing photo folders can be imported directly as albums, or a new album can be created and bound to a folder later; renaming, moving, or re-categorizing albums never affects the original files.
- **Search-first retrieval**: Timeline, smart search, conditional filters, person overview, and memories views locate a target photo within seconds.

Unlike cloud photo services (which require uploads, depend on the network, and charge by usage), this system is built on local storage and offline inference as its fundamental premise: photos and privacy data never leave the machine.

---

## 2. Features

| Module | Description |
| --- | --- |
| Album management | Create / edit / delete / rename / cover / tags / location / description; supports importing local folders |
| Multi-user isolation | Multiple accounts on one machine; albums, groups, tags, search, and sorting are isolated per user; passwords hashed with Argon2id + salt |
| Smart classification | Photos automatically grouped into 9 categories; animal sub-classification (dog / cat / bird); document OCR, scene recognition, and night-scene detection |
| Person recognition | Face detection and feature extraction; automatic clustering of the same person; rename, merge, and delete support; automatic person avatars |
| Timeline / memories | Cross-album photo timeline; aggregated memories by person, location, and holiday |
| Smart search | Combined search over title / tags / location / content; three-in-one conditional filters (EXIF + tone + AI) |
| Manual organization | Group (folder) tree; batch move / re-categorize; manual sorting; edit descriptions |
| Batch operations | Batch set location / tags, batch move to album, batch export to folder |
| Data security | Email / phone / password fields encrypted with AES-256-GCM; remember-login (3-day passwordless) |
| Offline capability | Thumbnails, person avatars, and GPS→province/city reverse geocoding are all performed locally, independent of the network |

---

## 3. Architecture

The system consists of three independent process layers and one independent microservice, collaborating through well-defined contracts:

```
┌─────────────────────────────────────────────────────────────┐
│  Frontend  Vue 3 + TypeScript + Vite + Pinia + Vue Router     │
│  (renderer process, WebView; UI rendering, state, commands)   │
└───────────────┬─────────────────────────────────────────────┘
                │  Tauri IPC (invoke commands / events)
                ▼
┌─────────────────────────────────────────────────────────────┐
│  Backend  Rust (Tauri main process)                           │
│  Command layer #[tauri::command] · business db/photo_scan/    │
│  thumbnail/tone/content/persons/auth/geo_index · SQLite        │
└───────────────┬─────────────────────────────────────────────┘
                │  HTTP (REST: HTTP client ← → 127.0.0.1:8765)
                ▼
┌─────────────────────────────────────────────────────────────┐
│  Microservice  Python VCR (visual recognition, standalone)    │
│  FastAPI + ONNX Runtime, layered: interface/service/          │
│  persistence/infrastructure                                   │
│  Models: YOLOv8 cls/det · SCRFD faces · ArcFace · PaddleOCR  │
└─────────────────────────────────────────────────────────────┘
```

Layer responsibilities:

- The **Rust backend** handles local I/O and security: file traversal, thumbnails, EXIF, SQLite, and cryptography.
- The **Python microservice** handles AI inference: the machine-learning ecosystem is concentrated in Python/ONNX and runs as a standalone process, avoiding a heavy inference stack inside Rust.
- The **Vue frontend** handles interaction through component-based development with the Composition API.

### Technology stack

| Layer | Technology |
| --- | --- |
| Desktop shell | Tauri 2.x (Rust) |
| Frontend | Vue 3 `<script setup>` + TypeScript + Vite 6 + Pinia + Vue Router 4 |
| Rust backend | rusqlite (embedded SQLite) / image / jpeg-decoder / kamadak-exif / walkdir / argon2 / aes-gcm / reqwest / tokio |
| Python microservice | FastAPI + uvicorn + onnxruntime + Pillow + numpy + opencv-python |
| AI models | YOLOv8s/n-cls (classification), YOLOv8n-det (COCO detection), SCRFD (face boxes), ArcFace (face embeddings), PaddleOCR (text) |

---

## 4. Module Design

### 4.1 Decoupling Principles

The architecture defines clear module boundaries so that each module can evolve, be replaced, or be removed independently:

1. **Process-level decoupling**: The Rust backend and the Python microservice communicate through a fixed REST contract (`POST /classify_batch`, `GET /health`, etc.) on a fixed port (`127.0.0.1:8765`). Both sides can be developed, tested, and packaged separately.
2. **Thin client**: `vision.rs` is a lightweight HTTP client and lifecycle manager. It does not depend on the `db` / `thumbnail` / `tone` modules (the image-extension list is duplicated locally to prevent implicit coupling); when the service is unavailable or models are missing, it returns explicit errors without affecting other features.
3. **Thin command layer**: `#[tauri::command]` handlers in `lib.rs` only perform argument passing, logging, and state injection; business logic lives in `db` / `content` / `persons` modules. `tauri::State` serves as the dependency-injection mechanism.
4. **Centralized configuration**: All paths, thresholds, model manifests, and GPU policies on the Python side are centralized in `python/vcr/config.py`; service layers reference configuration constants only.
5. **Automatic degradation**: When optional models (scene / flower / food / OCR) are missing, the corresponding channel degrades automatically; the primary classification channel is unaffected.

### 4.2 The VCR Microservice

VCR (Visual Content Recognition) is a standalone Python service that routes a photo through multiple model channels and arbitrates a final classification. It follows a classic layered architecture:

```
python/
├─ server.py                  # Interface layer: FastAPI routes + DTO (thin, no business logic)
├─ vcr/
│  ├─ config.py               # Infrastructure: paths / thresholds / model manifests
│  ├─ model_registry.py       # Infrastructure: model registry, lazy loading + GPU provider
│  ├─ preprocess.py           # Infrastructure: image preprocessing (resize / normalize)
│  ├─ mapping.py              # Infrastructure: ImageNet → 9-category mapping
│  ├─ taxonomy.py             # Infrastructure: taxonomy folding (output ∈ 9 categories)
│  ├─ schemas.py              # Interface layer: Pydantic DTOs
│  ├─ persistence/
│  │  └─ person_store.py      # Persistence layer: SQLite person registry
│  └─ services/               # Service layer: orchestration
│     ├─ classifier.py        #   classification channel (YOLOv8-cls)
│     ├─ detector.py          #   detection channel (YOLOv8-det: people / vehicles / objects)
│     ├─ face_service.py      #   face channel (SCRFD + ArcFace)
│     ├─ scene_service.py     #   scene channel (Places365, optional)
│     ├─ ocr_service.py       #   document channel (PaddleOCR, optional)
│     ├─ flower/food_service  #   expert channels (lazy-loaded, optional)
│     ├─ tone_service.py      #   tone channel (night / low-key detection)
│     ├─ arbitrator.py        #   arbitrator: multi-channel results → single conclusion
│     └─ pipeline.py          #   pipeline orchestration: decode once → multi-channel → arbitrate
```

Key design points:

- **Decode once, dispatch to many**: `pipeline.py` decodes each image only once and distributes tensors to the expert channels, avoiding repeated I/O.
- **Conditional expert channels**: Flower / food expert models are lazy-loaded and only loaded when classification hits their trigger conditions.
- **Arbitrator**: Each channel (classification, detection, scene, tone, OCR, expert) proposes candidates; the arbitrator combines them by threshold and priority into a single conclusion, guaranteeing the output category belongs to the 9-category taxonomy.
- **Automatic degradation**: Missing models cause the corresponding channel to degrade; `/health` reports the real readiness state.
- **Optional GPU**: The onnxruntime GPU provider (DirectML / CUDA) is auto-detected; GPU is used when available, with CPU fallback. `VCR_PROVIDER=cpu` forces CPU.

### 4.3 Frontend Component Reuse

The frontend achieves UI reuse through componentization and the Composition API:

- **Shared base components**: `Toast` / `ToastContainer` (global notifications), `ConfirmDialog`, `CollapseSection`, `PhotoGrid`, `PhotoLightbox`, `AlbumCard` / `AlbumMiniCard` are reused across home, album list, timeline, and memories views.
- **Complex view decomposition**: `AlbumDetail.vue` was refactored from 2709 lines down to 183 lines, decomposed into `AlbumMeta`, `PhotoGrid`, `CollapseSection`, and other sub-components, each with a single responsibility; the parent component only assembles data.
- **Centralized state**: Cross-view shared state (album / content / toast / theme / auth) lives in Pinia stores; components read and write through stores, avoiding prop drilling.
- **Composable reuse**: `useNotify.ts` encapsulates notification logic as a composable, callable in one line from any component.
- **Type-driven development**: `types/` defines `Album` / `Photo` / `Content` types that map one-to-one with backend serde JSON, keeping the contract clear so field changes surface at compile time.

---

## 5. Performance Optimizations

Notable optimizations implemented during development:

- **DCT downsampled thumbnail decoding**: Full-size decoding of 6000×4000 images takes 5–14 seconds; switching to `jpeg-decoder`'s `scale` (decoding only the needed DCT blocks) reduced cover-change time from 7.7s to 0.76s (~10×). Thumbnail caches are written to `app_data_dir/thumbs` and never written back to the database.
- **Batch content recognition**: `/classify_batch` processes up to 64 images per request (client default 8), with batched submission and real-time progress via `classify-progress` events.
- **GPU / batch acceleration**: The recognition service auto-detects GPU (DirectML) and uses it when available; otherwise CPU multi-threading is used (`THREADS=4`).
- **Offline GPS→province/city reverse lookup**: Embedded province/city boundary data enables offline point-in-polygon reverse geocoding, replacing per-photo online reverse encoding for both speed and privacy.
- **Combined scan**: EXIF (capture time), tone (exposure / night), and AI (content recognition) are completed in a single combined scan, reducing redundant traversal.
- **Pagination**: Photo grids render with pagination to avoid jank with large albums.
- **Multi-user security**: Passwords use Argon2id (GPU-brute-force resistant); email / phone / password hashes are encrypted at rest with AES-256-GCM.

---

## 6. Usage

### 6.1 Installation

- **Method 1 (recommended)**: Download an installer from the Release and run it:
  - `PhotoManagementSys_<version>_x64_en-US.msi`
  - or `PhotoManagementSys_<version>_x64-setup.exe` (NSIS)
- Installers include the AI models by default (see Section 7). If a given installer does not include the models, place the model files into the model directory (see the model-placement instructions) before starting the application.

### 6.2 First Use

1. **Register an account**: Register with a username / email / phone and a password (multi-user on the same machine; albums are isolated per user).
2. **Import photos**: On the Album Management page, create an album and select a local photo folder. The system scans images in it (jpg / jpeg / png / webp / gif / bmp are supported).
3. **Start AI recognition**: On the album detail page, run Content Recognition / Combined Scan. When the progress bar completes, each photo is automatically categorized (animals / food / buildings / night scenes / portraits, etc.) and people are clustered automatically.

### 6.3 Daily Use

- **Timeline**: Browse all photos chronologically across albums.
- **Smart search**: Enter keywords or combine conditional filters (EXIF capture time / tone / AI category) for precise retrieval.
- **Person overview**: Open the People page to rename, merge (multiple faces of the same person), or delete false detections.
- **Manual organization**: Create folder trees under Groups and batch-move albums into them; adjust order under Manual Sort; batch-set locations / tags for albums.
- **Batch export**: Select multiple photos and export them to a target folder in one click.
- **Memories**: Open the Memories view to see aggregations by person, location, and holiday.

### 6.4 Privacy & Security

- Photos, thumbnails, and person data all reside under the local `app_data_dir` and are never uploaded.
- For cross-machine migration, copy the photo folder and re-import it; album metadata can be migrated together with `photos.db`.

---

## 7. Build & Release

### 7.1 Dependency Notes

- **Main application**: `PhotoManagementSys.exe`, the Tauri build artifact (embeds the Rust backend and the Vue frontend).
- **VCR microservice**: Packaged independently as `vcr-server.exe` (PyInstaller single file, ~85 MB, includes the Python runtime and dependencies), embedded into the installer under `vcr/`.
- **AI models**: `.onnx` model files are large and are not committed to the git repository (generated by download/export scripts); they are embedded into the installer under `vcr/models/`, or can be placed by the user per the instructions.

### 7.2 Model Placement

Installers include the models by default. If a given installer does not, verify that the model directory contains the following files:

```
vcr/models/
├─ yolov8n-cls.onnx        # classification (required)
├─ yolov8n-det.onnx        # COCO detection (required)
├─ det_500m.onnx           # SCRFD face detection (required)
├─ w600k_mbf.onnx          # ArcFace face embeddings (required)
├─ paddleocr-det.onnx      # document OCR (optional)
├─ album_groups.json       # 9-category definitions
├─ imagenet_classes.txt    # ImageNet class names
└─ imagenet_to_album.json  # ImageNet → category mapping
```

Optional models (degrade automatically when missing): `resnet18_places365.onnx` (scene), `efficientnet-b2-flowers.onnx` (flower expert).

### 7.3 Obtaining / Updating Models

- **Face + OCR**: run `python/download_models.py` (downloads from GitHub / ModelScope).
- **Classification + detection**: export with `ultralytics` (`python/export_model.py` for classification; similarly for detection), or copy the corresponding `.onnx` files from the `python/models/` directory of a development machine.
- Models are not committed to git (too large); obtain them via GitHub Release attachments (model zip) or the links in this README.

### 7.4 Release Build

Run the following at the project root:

```powershell
powershell -ExecutionPolicy Bypass -File release.ps1
```

The script performs: ① build `python/dist/vcr-server.exe` in a minimal venv → ② verify / complete models → ③ `npm install` → ④ `npx tauri build --config src-tauri/release.tauri.conf.json` (merges the release config, embedding the microservice and models into the installers).

Output location: `src-tauri/target/release/bundle/msi/` and `nsis/`.

---

## 8. Development Environment

### 8.1 Dependencies

- Node.js 18+, npm
- Rust + Cargo (MSVC build tools required)
- Python 3.10+ (for VCR microservice development)
- Tauri CLI: `npm run tauri` or `cargo install tauri-cli`

Optional one-click environment script (Windows): `setup-env.ps1` (run as administrator; installs Rust, C++ Build Tools, and mirrors).

### 8.2 Getting Started

```bash
# 1. Install frontend dependencies
npm install

# 2. Install VCR microservice dependencies (development mode, system Python)
pip install -r python/requirements.txt -i https://pypi.tuna.tsinghua.edu.cn/simple

# 3. Download models (face / OCR)
python python/download_models.py

# 4. Start everything (`tauri dev` launches the frontend and Rust; the microservice is started on demand by Rust)
npm run tauri dev
```

In development mode, Rust falls back to starting the microservice via `python server.py` (see `vision.rs`); the packaged version starts the embedded `vcr-server.exe`.

---

## 9. Directory Structure

```
PhotoMangementSys/
├─ src/                         # Vue 3 frontend
│  ├─ views/                    #   route views (album list / detail / timeline / memories / search…)
│  ├─ components/               #   reusable components (PhotoGrid / Toast / AlbumCard…)
│  ├─ stores/                   #   Pinia state (album / content / toast / theme / auth)
│  ├─ composables/              #   composables (useNotify)
│  ├─ router/  utils/  types/   #   router / utilities / types
├─ src-tauri/                   # Rust backend + Tauri shell
│  ├─ src/                      #   lib.rs (commands / assembly) + db / thumbnail / vision / …
│  ├─ tauri.conf.json           #   main configuration
│  └─ release.tauri.conf.json   #   release-only configuration (embedded microservice + model resources)
├─ python/                      # VCR visual recognition microservice
│  ├─ server.py                 #   FastAPI interface layer
│  ├─ vcr/                      #   service / persistence / infrastructure
│  ├─ models/                   #   models (not in git)
│  ├─ build_vcr_exe.ps1         #   builds vcr-server.exe in a minimal venv
│  └─ requirements.txt          #   runtime dependencies
├─ release.ps1                  # one-click release build (exe + models + MSI)
├─ package.json / vite.config.ts
└─ README.md
```

---

## 10. FAQ

**Q: `/health` reports GPU unavailable?**
The packaged version uses CPU inference by default. For GPU, repackage with an onnxruntime that includes the DirectML / CUDA provider; in development, `pip install onnxruntime-directml` and set `VCR_PROVIDER=auto`.

**Q: After changing computers, can photos in albums still be found?**
Photos are local files bound only to folder paths. Copy the photo folder to the new machine and re-import; for tag / classification data, migrate `photos.db` as well (same version required).

**Q: The recognition service did not start / reports "models not loaded"?**
Check that the corresponding `.onnx` files exist under `vcr/models/`; if not, obtain them per Section 7. In development, install `python/requirements.txt` first.

---

## 11. License

Private project. Author: haoyuan.
