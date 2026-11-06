# Chạy benchmark hiệu năng

Crate `tafit-perf` chứa các benchmark Criterion chạy qua store và backend gix thật, cùng báo cáo so kết quả với ngân sách hiệu năng (`crates/tafit-perf/src/budgets.rs`).

## Tập PR (mặc định)

Mở một repo fixture tạo bằng git CLI, đo CPU idle với 1 và 10 Repository, đo chuyển Repository Tab khi có 20 tab đang mở (từ `Msg::ActivateRepo` tới lúc store công bố snapshot mới, chưa tính vẽ lại UI), và đo refresh status trên một repo fixture 80 nghìn file có thay đổi (đã stage, chưa stage, untracked). Mất vài phút sau khi build xong, phần lớn là để dựng repo 80 nghìn file.

```bash
cargo bench -p tafit-perf
```

```bash
cargo run -p tafit-perf --bin budget-report
```

Báo cáo in một bảng Markdown và thoát với mã khác 0 khi có benchmark vượt ngân sách hoặc thiếu kết quả.

Criterion ghi kết quả vào `target/criterion` và giữ lại kết quả của các lần chạy trước. Báo cáo đọc mọi kết quả trong thư mục đó, nên nếu đã đổi tên hay xoá benchmark thì nên dùng một thư mục mới:

```bash
CRITERION_HOME=/tmp/tafit-criterion cargo bench -p tafit-perf
```

```bash
CRITERION_HOME=/tmp/tafit-criterion cargo run -p tafit-perf --bin budget-report
```

Chỉ chạy một benchmark: `cargo bench -p tafit-perf --bench idle -- idle_cpu/1_repo`.

## Bộ đầy đủ (như job hằng đêm)

Thêm đo RSS tăng qua 10 phút idle (khoảng 11 phút) và mở rồi đọc status các snapshot repo thật: Git, Bun và Chromium. Snapshot là bản clone blobless toàn bộ lịch sử tới một tag cố định; Chromium chiếm nhiều GB, nên tải một lần rồi dùng lại.

```bash
./scripts/perf-snapshots.sh ~/tafit-perf-snapshots
```

```bash
TAFIT_PERF_SUITE=nightly TAFIT_PERF_SNAPSHOTS=~/tafit-perf-snapshots cargo bench -p tafit-perf
```

```bash
cargo run -p tafit-perf --bin budget-report -- --nightly
```

Chỉ đặt `TAFIT_PERF_SNAPSHOTS` (không có `TAFIT_PERF_SUITE`) thì tập PR mở thêm những snapshot có sẵn trong thư mục, không bắt buộc đủ cả ba.

## Trong CI

- Mỗi PR: job `Performance budgets` trong `ci.yml` chạy tập PR trên `ubuntu-24.04`.
- Mỗi đêm: `perf-nightly.yml` chạy bộ đầy đủ. Snapshot được cache theo hash của `scripts/perf-snapshots.sh`, chỉ clone lại khi đổi tag. Đặt biến repository `PERF_RUNNER` thành label của runner riêng để số đo ổn định giữa các đêm và đủ chỗ cho Chromium.

## Thêm benchmark

1. Thêm benchmark vào `crates/tafit-perf/benches/` (hoặc file mới, kèm `[[bench]]` với `harness = false` trong `Cargo.toml`). Dùng `harness::Store` để đi qua store thật.
2. Thêm ngân sách vào `BUDGETS`, chọn `Suite::Pr` nếu PR cần chạy nó. Ngân sách đặt theo id benchmark (`idle_cpu/1_repo`) hoặc theo nhóm (`open_repository`).
3. Benchmark có ngân sách thuộc lần chạy mà không ra kết quả sẽ làm báo cáo thất bại, nên đổi tên benchmark thì phải đổi cả ngân sách.
