# Makes the test videos (needs ffmpeg on PATH or -Ffmpeg): talk.mp4 (slides
# with a moving frame counter, sound), muted.mp4 (the same, silent) and
# static.mp4 (a still picture with sound, like music over cover art).
param([string]$Out = "$PSScriptRoot\out", [string]$Ffmpeg = "ffmpeg")
New-Item -ItemType Directory -Force $Out | Out-Null
Copy-Item "$PSScriptRoot\video.html" $Out -Force
$font = "C\:/Windows/Fonts/segoeui.ttf"
$slides = "drawtext=fontfile='$font':text='Quarterly results at a glance':fontsize=44:fontcolor=white:x=60:y=80,drawtext=fontfile='$font':text='Revenue grew eighteen percent year over year':fontsize=30:fontcolor=white:x=60:y=180,drawtext=fontfile='$font':text='Three new regional offices opened':fontsize=30:fontcolor=white:x=60:y=230,drawtext=fontfile='$font':text='Frame %{n}':fontsize=26:fontcolor=yellow:x=60+mod(n*7\,500):y=330"
$still = "drawtext=fontfile='$font':text='Lo-fi beats for focus':fontsize=46:fontcolor=white:x=60:y=120,drawtext=fontfile='$font':text='Side A - Track 3 - Morning rain':fontsize=30:fontcolor=0xbbbbbb:x=60:y=210"
& $Ffmpeg -y -loglevel error -f lavfi -i "color=c=0x0f2a43:s=960x540:r=30:d=60" -f lavfi -i "sine=frequency=330:duration=60" -vf $slides -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest "$Out\talk.mp4"
& $Ffmpeg -y -loglevel error -f lavfi -i "color=c=0x0f2a43:s=960x540:r=30:d=60" -vf $slides -c:v libx264 -pix_fmt yuv420p -an "$Out\muted.mp4"
& $Ffmpeg -y -loglevel error -f lavfi -i "color=c=0x1d1d1d:s=960x540:r=30:d=60" -f lavfi -i "sine=frequency=262:duration=60" -vf $still -c:v libx264 -tune stillimage -pix_fmt yuv420p -c:a aac -shortest "$Out\static.mp4"
Get-ChildItem $Out | Select-Object Name, Length
