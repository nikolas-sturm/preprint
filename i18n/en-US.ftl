# Preprint - English (United States)

# Top bar
tagline = Prepare photos for print
export-all = Export all
exporting = Exporting…
preview = Preview
rendering = Rendering…
language = Language
switch-to-light = Switch to light mode
switch-to-dark = Switch to dark mode

# Status strip
files-loaded = { $count ->
    [one] { $count } file loaded
   *[other] { $count } files loaded
}
loaded-images = { $count ->
    [one] Loaded { $count } image
   *[other] Loaded { $count } images
}

# Input files card
card-input-files = INPUT FILES
hint-drag-drop = drag & drop supported
add-images = Add images…
drop-to-load = Drop to load images
no-images = No images loaded
release-to-import = Release the mouse button to import.
add-or-drag = Click {"\""}Add images…{"\""} above, or drag files here.
image-fallback-name = Image

# Print setup card
card-print-setup = PRINT SETUP
target-size = Target size
border-width = Border width
border-style = Border style
border-white = White
border-black = Black
border-mirrored-blur = Mirrored blur

# PPI
ppi-select-image = Select an image to see its PPI.
ppi-inspecting = Inspecting image…
ppi-dimensions-unavailable = Dimensions unavailable.
ppi-quality-low = low — likely soft
ppi-quality-ok = acceptable for print
ppi-quality-sharp = sharp
ppi-aspect-warning = Target aspect ratio differs from the source — the image will be cropped or padded.

# Output card
card-output = OUTPUT
save-to = Save to
change = Change…
choose = Choose…
no-folder = No folder chosen
format = Format
quality = Quality
bit-depth = Bit depth
effort = Effort
compression = Compression
deflate-fast = Fast
deflate-balanced = Balanced
deflate-best = Best
bit-depth-8 = 8-bit
bit-depth-16 = 16-bit
source-other-depth = other bit depth
note-16-tiff-available = 16-bit TIFF is available for the selected 16-bit source.
note-16-tiff-8-source = 16-bit TIFF is disabled: the selected source is only 8-bit, and we don't invent extra detail.
note-16-tiff-needs-source = 16-bit TIFF needs a source that decodes as true 16-bit.
note-16-tiff-only = 16-bit output is only available for TIFF.

# Export status card
card-export-status = EXPORT STATUS
no-export-running = No export running yet.
export-started = { $count ->
    [one] Export started — { $count } worker
   *[other] Export started — { $count } workers
}

# Preview summary card
card-preview = PREVIEW
softproof-profile = Softproof profile
clear = Clear
profile-fallback = profile
no-softproof = None — no softproof
preview-window-hint = Preview opens in its own window — resize it freely or go fullscreen.
profile-change-hint = Changing the profile needs a preview refresh to take effect.
rendering-background = Rendering preview in background…
preview-size = Preview size: { $width } × { $height } px
refresh-preview = Refresh preview
show-window = Show window
cleared-softproof = Cleared softproof profile.

# Progress / status messages
idle = Idle
starting-preview = Starting preview
preview-ready = Preview ready
select-image-to-preview = Select an image to preview first
pick-output-folder = Pick an output folder first
add-images-to-export = Add images to export first
loading-image = Loading image
applying-border = Applying border
simulating-compression = Simulating compression
applying-icc = Applying ICC softproof
preparing-texture = Preparing texture

# File entry
entry-inspecting = Inspecting…
entry-error = Error: { $error }
entry-dimensions-depth = { $width } × { $height }px · { $depth }
entry-dimensions = { $width } × { $height }px
entry-ready = Ready

# Preview window
preview-title = Preprint — Preview
softproof-on = Softproof on
softproof-off = Softproof off
no-softproof-profile = No softproof profile
magnifier = Magnifier
zoom = Zoom
lens = Lens
magnifier-hint = Hold left mouse button on the image to inspect.
close = Close
fullscreen = Fullscreen
exit-fullscreen = Exit fullscreen
no-preview-yet = No preview rendered yet — click {"\""}Refresh preview{"\""}.

# Compression preview labels
compression-jpeg = Compression preview: JPEG q{ $quality }
compression-png = Compression preview: PNG effort { $level }
compression-tiff-lzw = Compression preview: TIFF LZW
compression-tiff-zip = Compression preview: TIFF ZIP ({ $level })
