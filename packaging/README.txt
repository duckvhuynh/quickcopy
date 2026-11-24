QuickCopy
=========

Copy and translate the text in any image or paused video on your screen.
https://github.com/duckvhuynh/quickcopy

Start
-----
Run quickcopy.exe. Its settings window opens; close it and QuickCopy keeps
running in the notification area (tray). Rest the mouse on an image for a
moment, then drag over its text to select it, and press Ctrl+C to copy or
Ctrl+T to translate. Double-click the tray icon, or run quickcopy.exe
again, to open the settings.

Keep this folder somewhere you can write to, for example under
%LOCALAPPDATA%, not in Program Files: updates replace quickcopy.exe here.

Vietnamese text recognition
---------------------------
Windows OCR cannot read Vietnamese, so QuickCopy uses the VieSVTR model by
ngocthua830:
https://huggingface.co/ngocthua830/viesvtr

The model is not in this zip. The first time QuickCopy runs, it downloads
it (30.7 MB) from that page, checks its SHA-256 hash and saves it to
%LOCALAPPDATA%\QuickCopy\models\viesvtr\viesvtr_v7.onnx. If there is no
internet connection then, it tries again at the next start, or when you
select Retry in the settings.

The model is licensed CC BY-NC 4.0
(https://creativecommons.org/licenses/by-nc/4.0/): you may use it for
non-commercial purposes only. To stop using it, turn off "Vietnamese text
recognition" in the settings, then delete that file.

On Windows 10, also put onnxruntime.dll (version 1.17 or newer) next to
quickcopy.exe. Windows 11 already has it.

Updates
-------
QuickCopy checks GitHub for a new version once a day and shows
"Update to X.Y.Z" in the tray menu and the settings. It checks the
download's SHA-256 hash before installing. Turn this off with
"Check for updates automatically" in the settings.

Files
-----
quickcopy.exe                       the program
models\viesvtr\vi_syllables.bin     Vietnamese syllable frequencies that pick
                                    tone marks (from Vietnamese Wikipedia,
                                    CC BY-SA 4.0)
LICENSE-MIT, LICENSE-APACHE         QuickCopy's license (either one)
THIRD-PARTY-NOTICES.md              what QuickCopy uses from others

Settings are in %APPDATA%\QuickCopy. To uninstall, quit QuickCopy from the
tray menu, turn off "Start with Windows" first if it is on, and delete this
folder, %APPDATA%\QuickCopy and %LOCALAPPDATA%\QuickCopy.


--- Tiếng Việt ---

QuickCopy sao chép và dịch chữ trong ảnh hoặc video đang tạm dừng trên màn
hình.

Chạy quickcopy.exe, cửa sổ cài đặt sẽ mở ra; đóng cửa sổ thì ứng dụng vẫn
chạy ở khay hệ thống. Để chuột yên trên ảnh một chút, kéo chọn chữ, nhấn
Ctrl+C để sao chép hoặc Ctrl+T để dịch. Nhấn đúp biểu tượng ở khay, hoặc
chạy lại quickcopy.exe, để mở cài đặt. Hãy để thư mục này ở nơi ghi được (ví dụ
trong %LOCALAPPDATA%), không để trong Program Files, vì bản cập nhật thay
quickcopy.exe tại đây.

Nhận dạng tiếng Việt dùng model VieSVTR của ngocthua830
(https://huggingface.co/ngocthua830/viesvtr). Model không có trong gói zip:
lần đầu chạy, QuickCopy tự tải model (30,7 MB) từ trang trên, kiểm tra mã
SHA-256 và lưu vào %LOCALAPPDATA%\QuickCopy\models\viesvtr\. Nếu lúc đó
không có mạng, ứng dụng tải lại ở lần chạy sau, hoặc khi bấm "Thử lại"
trong cài đặt. Model dùng giấy phép CC BY-NC 4.0: chỉ được dùng cho mục
đích phi thương mại. Trên Windows 10 cần thêm onnxruntime.dll (1.17 trở
lên) đặt cạnh quickcopy.exe; Windows 11 đã có sẵn.

QuickCopy kiểm tra bản mới trên GitHub mỗi ngày và hiện "Cập nhật lên
X.Y.Z" ở menu khay và trong cài đặt; tệp tải về được kiểm tra mã SHA-256
trước khi cài. Có thể tắt bằng "Tự động kiểm tra cập nhật".

Gỡ cài đặt: thoát QuickCopy từ menu khay (tắt "Khởi động cùng Windows"
trước nếu đang bật), rồi xóa thư mục này, %APPDATA%\QuickCopy và
%LOCALAPPDATA%\QuickCopy.
