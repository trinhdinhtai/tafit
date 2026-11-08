# State theo mô hình MVU, nằm ngoài UI framework, với ranh giới crate được CI kiểm tra

Toàn bộ state của app nằm trong một store MVU (model-view-update) ở crate `state`, crate này không phụ thuộc UI framework. Mọi thay đổi đi qua `Msg` → reducer thuần → snapshot `AppState` bất biến kèm danh sách `Effect`. Effect (thao tác git, đọc ghi file) chạy trên các thread pool riêng, nhận `CancellationToken`, và gửi kết quả về store bằng một `Msg` mới. UI chỉ đọc snapshot rồi vẽ lại. Chúng ta chọn cách này thay cho cách thông thường của GPUI (lưu state trong entity) để: reducer test được mà không cần UI framework, UI thread không bao giờ bị chặn bởi git, kết quả cũ bị loại bỏ nhờ epoch/request id, và có thể chạy headless (mergetool, CLI). Ranh giới được CI kiểm tra bằng `cargo tree`: `core` và `state` không được phụ thuộc UI framework, bản build headless không được kéo UI framework vào.

## Các phương án đã cân nhắc

- **State trong GPUI entity (cách của Zed)**: ít tầng hơn, nhưng logic gắn chặt vào GPUI, khó test và không chạy headless được.
