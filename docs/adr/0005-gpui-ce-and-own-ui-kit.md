# Dùng `gpui-ce` và tự viết bộ widget `ui-kit`, không dùng `gpui-kit`

tafit dùng fork `gpui-ce` (Apache-2.0), khai báo là git dependency pin theo một rev cố định, và tự viết toàn bộ widget trong crate `ui-kit` (button, tabs, menu, modal, toast, picker, text input, scrollbar, minimap, theme token, icon, font, window chrome). `ui-kit` không được phụ thuộc `core` hay `state`. Chúng ta chọn cách này để kiểm soát hoàn toàn hiệu năng và hành vi của từng widget, và để được tự chọn bản GPUI (có renderer wgpu trên Windows) thay vì phải theo bản GPUI đi kèm một thư viện widget.

## Các phương án đã cân nhắc

- **`gpui-kit` / `gpui-component` (longbridge, Apache-2.0)**: rất nhiều widget có sẵn, có editor và cộng đồng lớn, nhưng kéo theo bản GPUI riêng của họ (`gpui-pre`/`gpui-fast`). Vì vậy không dùng chung được với `gpui-ce`, và tafit sẽ phải theo lịch phát hành của họ.
- **`gpui` chính thức trên crates.io**: là thượng nguồn, nhưng chậm nhận các bản vá cần cho một app không phải Zed.

## Hệ quả

- Phải tự viết và tự bảo trì widget, gồm cả focus, bàn phím, IME (tiếng Việt) và accessibility.
- `gpui-ce` chỉ có một người duy trì. Nếu fork ngừng cập nhật, tafit phải tự duy trì fork hoặc chuyển sang `gpui` chính thức. Việc chuyển này tương đối nhẹ vì API gần như giống nhau.
