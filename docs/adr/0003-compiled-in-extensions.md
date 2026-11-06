# Extension được biên dịch thẳng vào app (Rust trait), không nạp lúc chạy

Extension của tafit là một crate Rust cài đặt trait `Extension` và được đăng ký trong code khi khởi động app. Extension đóng góp lệnh, phím tắt, menu, tab, panel, trang cài đặt và trang trí cho diff pane, và gọi GPUI trực tiếp. Tập extension được kiểm tra rồi đóng băng trước khi cửa sổ đầu tiên mở. Chúng ta chọn cách này vì nó đơn giản nhất, cho extension sức mạnh ngang code của host, và đã được kiểm chứng trong các Git GUI Rust + GPUI khác. Hệ quả: không có cài extension lúc chạy, không có sandbox, và API không có version riêng. Người viết extension bên thứ ba phải build lại app cùng revision, và license MIT/Apache-2.0 (ADR-0001) cho phép họ làm vậy mà không bị ràng buộc copyleft.

## Các phương án đã cân nhắc

- **WASM component (wasmtime + WIT), UI dạng khai báo**: cài lúc chạy, có sandbox, API có version, nhưng extension yếu hơn và tốn công làm nhiều hơn. Có thể thêm sau như một lớp nằm trên các trait hiện có.
- **Plugin native (dylib)**: Rust không có ABI ổn định và không có sandbox.
