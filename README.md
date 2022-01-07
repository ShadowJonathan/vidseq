# `vidseq`

> a slightly scuffed interface to gstreamer to "just" grab induvidual frames from video files

Note: This library initiates gstreamer by itself, call `assume_gst_init` before everything else if gst is already initiated somewhere else.

---

required packages:
```
libgstreamer1.0-dev
libgstreamer-plugins-base1.0-dev
```

recommended packages:
```
gstreamer1.0-plugins-base
gstreamer1.0-plugins-good
gstreamer1.0-plugins-bad
gstreamer1.0-plugins-ugly
```