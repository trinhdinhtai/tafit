# Dùng Rune và `rune-kit` cho UI, thay `gpui-ce` và `ui-kit` tự viết

Thay thế ADR-0005.

tafit dùng [Rune](https://github.com/anomalyco/rune) (MIT) làm UI framework và dùng thẳng bộ component `rune-kit` (diff view, file tree, tab strip, menu, text area, toast…), thay cho `gpui-ce` cộng bộ widget `ui-kit` tự viết. Rune là git dependency pin theo một rev cố định của nhánh `dev`. Rev chỉ được nâng trong một commit `build:` riêng. Lý do chính là `rune-kit` đã có sẵn gần như mọi widget một Git GUI cần, kể cả diff view có chọn dòng và virtual list, trong khi ADR-0005 bắt tafit tự viết và tự bảo trì toàn bộ, gồm cả focus, bàn phím, IME và accessibility.

Trước khi chuyển, một prototype (issue #36) đã qua cổng trên Windows với 4 tiêu chí: gõ tiếng Việt bằng Unikey/EVKey đúng, danh sách 10k dòng cuộn mượt, `Diff` hiển thị đúng một diff thật của tafit, và vùng chọn dòng của `Diff` điều khiển được từ store.

## Các phương án đã cân nhắc

- **Giữ `gpui-ce` và `ui-kit` (ADR-0005)**: ổn định và đã chạy, nhưng tafit phải tự viết diff view, tree, menu, tab và phần accessibility mà `rune-kit` đã có.
- **`gpui-component`**: vẫn bị loại vì lý do trong ADR-0005 (kéo theo bản GPUI riêng).
- **Chạy song song `ui-gpui` và `ui-rune` lâu dài**: an toàn hơn khi chuyển, nhưng phải duy trì hai UI. Hai framework đều muốn sở hữu vòng lặp sự kiện và cửa sổ, nên cũng không thể trộn từng màn hình.

## Hệ quả

- Rune đang ở bản pre-release và không nhận pull request. Gặp bug thì mở issue upstream. Nếu bị chặn quá lâu thì tafit tự fork (license MIT cho phép).
- Bỏ crate `ui-kit` và luật CI "`ui-kit` không phụ thuộc `core`/`state`". Theme và font tiếng Việt nằm trong crate UI. Theme riêng của tafit (dạng `Theme::from_json`) làm sau.
- MVU của ADR-0004 giữ nguyên. Component nào của `rune-kit` tự giữ state mà state đó là khái niệm domain (ví dụ vùng chọn dòng của `Diff` chính là Line Selection) thì component chỉ giữ bản sao. Thao tác của người dùng dispatch `Msg`, và giá trị từ snapshot được đưa lại vào component.
- Diff view dùng thẳng `Diff` của `rune-kit` và chấp nhận các thay đổi hành vi sau:
  - Người dùng chọn dòng bằng một dải liên tục (click, Shift-click, kéo trên số dòng), không còn bật/tắt từng dòng rời rạc. Line Selection trong store vẫn là một tập dòng.
  - Không còn nút Stage/Unstage/Discard trên từng hunk. Muốn áp dụng cho một hunk thì chọn các dòng của hunk đó.
  - Tab luôn dừng ở bội số của 4 cột, nên bỏ tuỳ chọn tab size.
  - Hai cột luôn cuộn cùng nhau và luôn wrap, nên bỏ tuỳ chọn cuộn riêng.
  - Store luôn đọc diff dạng hunk. Hai cột là cách `rune-kit` vẽ lại cùng các hunk đó, không còn căn dòng toàn file bằng patience/Myers của tafit.
  - Hunk của tafit được đưa vào `rune-kit` dưới dạng text patch (`diff::parse`), vì `rune-kit` không nhận hunk dựng từ bên ngoài. Word change do `rune-kit` tự tính.
- Extension (ADR-0003) gọi Rune thay vì GPUI.
- Rune có chế độ headless (render ra PNG, dump cây element, chạy input bằng script). Việc dùng nó để test UI là một ticket riêng.
- Build trên Linux cần `libxkbcommon-dev` và `libwayland-dev`.
