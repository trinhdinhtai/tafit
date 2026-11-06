# tafit

Ứng dụng desktop hiệu năng cao để làm việc với Git: xem lịch sử, diff, giải quyết conflict và thực hiện các thao tác Git hằng ngày trên nhiều repo cùng lúc.

## Repo và cửa sổ

**Repository**:
Một repo Git (thường, bare, hoặc một worktree) mà người dùng đã mở trong app.
_Avoid_: Project, repo folder

**Repository Tab**:
Tab trong cửa sổ hiển thị đúng một Repository; mỗi Repository đang mở có đúng một Repository Tab.
_Avoid_: Workspace, view

**Active Repository**:
Repository của Repository Tab đang được chọn; chỉ Active Repository được theo dõi thay đổi file liên tục.
_Avoid_: Current repo, focused repo

**Window**:
Cửa sổ của app, chứa một nhóm Repository Tab và được lưu/khôi phục cùng nhau.
_Avoid_: Workspace

**Worktree**:
Một working tree liên kết (linked worktree) của cùng một Repository; có thể mở thành Repository Tab riêng.
_Avoid_: Checkout, clone

**Submodule**:
Repository con được Repository cha tham chiếu tới một commit cố định; có thể mở thành Repository Tab riêng.
_Avoid_: Nested repo, subrepo

**Session**:
Toàn bộ những gì app ghi nhớ giữa các lần chạy: các Repository đang mở, Window, kích thước panel, lựa chọn hiển thị và cài đặt người dùng.
_Avoid_: Settings, preferences, workspace state

## Thay đổi và commit

**Line Selection**:
Tập các dòng thay đổi người dùng chọn (có thể trải qua nhiều hunk) để stage, unstage hoặc discard.
_Avoid_: Partial hunk, line staging

**Discard**:
Bỏ thay đổi trong working tree của một file, hunk hoặc Line Selection; không thể hoàn tác qua git.
_Avoid_: Revert, reset, rollback

**Hook Activity**:
Tiến trình và output của git hook đang chạy trong một thao tác, hiển thị riêng cho người dùng.
_Avoid_: Hook log, hook output

## Đồng bộ với remote

**Upstream**:
Nhánh remote mà một nhánh local được cấu hình để theo dõi; là đích mặc định của Pull và Push.
_Avoid_: Tracking branch, remote branch

**Pull Mode**:
Cách Pull tích hợp thay đổi từ Upstream vào nhánh đang checkout: theo git config, Merge, Fast-forward nếu được, Fast-forward only, hoặc Rebase.
_Avoid_: Pull strategy

**Fetch All**:
Thao tác do người dùng chủ động, tải về mọi remote của Repository mà không thay đổi nhánh local hay working tree.
_Avoid_: Sync, refresh

**Auto-fetch**:
Fetch All do app tự chạy ngầm theo lịch, không bao giờ merge, rebase hay checkout, và không bao giờ hỏi thông tin đăng nhập.
_Avoid_: Background sync, auto-pull

**Incoming Commits**:
Số commit có trên Upstream mà nhánh đang checkout chưa có, tính theo lần fetch gần nhất.
_Avoid_: Behind count

## Diff

**Diff Engine**:
Cách tính sự khác biệt giữa hai phiên bản file: Line Diff hoặc Structural Diff.
_Avoid_: Diff algorithm, differ

**Line Diff**:
Diff Engine mặc định, so sánh theo từng dòng văn bản.
_Avoid_: Text diff, plain diff

**Structural Diff**:
Diff Engine so sánh theo cấu trúc cú pháp của mã nguồn; tự quay về Line Diff khi không xử lý được file.
_Avoid_: Syntax diff, semantic diff, difftastic diff

## Lịch sử

**History Mode**:
Quy tắc chọn commit nào hiện trong lịch sử: toàn bộ commit reachable, chỉ first-parent của nhánh hiện tại, bỏ merge, chỉ merge, hoặc mọi nhánh.
_Avoid_: History filter, log mode

**Commit Graph**:
Hình vẽ các lane nối commit với parent của nó, hiển thị cạnh danh sách lịch sử.
_Avoid_: Branch graph, tree view

**Hidden Ref**:
Ref thuộc namespace bị ẩn, không bao giờ là điểm bắt đầu duyệt lịch sử.
_Avoid_: Ignored ref

## Conflict

**Conflict Kind**:
Loại conflict của một file: cả hai cùng sửa, cả hai cùng thêm, một bên xóa còn bên kia sửa/thêm, cả hai cùng xóa, hoặc file nhị phân.
_Avoid_: Conflict type, conflict status

**Three-Way Resolver**:
Công cụ giải quyết conflict văn bản dựa trên Base, Ours và Theirs, cho phép chọn từng hunk hoặc sửa tay kết quả; dùng cho "cả hai cùng sửa" và "cả hai cùng thêm" (khi đó Base có thể rỗng).
_Avoid_: 3-way merge view, merge editor

**Two-Way Resolver**:
Công cụ giải quyết conflict khi một bên không có file, chỉ cho phép quyết định giữ hay xóa file.
_Avoid_: 2-way merge, ours/theirs view

**Autosolve**:
Việc app tự giải quyết những hunk conflict mà kết quả là hiển nhiên, ví dụ hai bên giống nhau hoặc chỉ một bên thay đổi.
_Avoid_: Auto-merge, auto-resolve

**Mergetool Mode**:
Chế độ app được git gọi qua `git mergetool` để giải quyết conflict của đúng một file, rồi báo kết quả lại cho git bằng exit code.
_Avoid_: External merge tool, merge mode
