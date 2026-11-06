# Viết mới từ đầu với license MIT/Apache-2.0

tafit được viết từ đầu và phát hành dưới license MIT/Apache-2.0, vì hai lý do. Thứ nhất, license dễ dãi cho phép bên thứ ba viết extension và nhúng app mà không bị ràng buộc copyleft. Thứ hai, chúng ta muốn tự thiết kế kiến trúc core theo mục tiêu hiệu năng của riêng mình. Các Git GUI khác chỉ được dùng để tham khảo hành vi sản phẩm và ý tưởng thiết kế; không được copy code có license không tương thích (ví dụ GPL/AGPL) vào repo này.

## Các phương án đã cân nhắc

- **Fork một Git GUI Rust + GPUI có sẵn**: nhanh nhất để có đủ tính năng, nhưng phải nhận license copyleft của dự án gốc (có thể lan sang cả extension chạy cùng tiến trình) và gánh kiến trúc của họ.
- **Đóng góp vào một dự án có sẵn**: bị loại vì cùng lý do license và kiến trúc.
