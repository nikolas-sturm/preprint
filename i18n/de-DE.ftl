# Preprint - Deutsch (Deutschland)

# Top bar
tagline = Fotos für den Druck vorbereiten
export-all = Alle exportieren
exporting = Exportiere…
preview = Vorschau
rendering = Rendere…
language = Sprache
switch-to-light = Zum hellen Modus wechseln
switch-to-dark = Zum dunklen Modus wechseln

# Status strip
files-loaded = { $count ->
    [one] { $count } Datei geladen
   *[other] { $count } Dateien geladen
}
loaded-images = { $count ->
    [one] { $count } Bild geladen
   *[other] { $count } Bilder geladen
}

# Input files card
card-input-files = EINGABEDATEIEN
hint-drag-drop = Drag & Drop unterstützt
add-images = Bilder hinzufügen…
drop-to-load = Zum Laden ablegen
no-images = Keine Bilder geladen
release-to-import = Maustaste loslassen, um zu importieren.
add-or-drag = Oben auf {"\""}Bilder hinzufügen…{"\""} klicken oder Dateien hierher ziehen.
image-fallback-name = Bild

# Print setup card
card-print-setup = DRUCKEINSTELLUNGEN
target-size = Zielgröße
border-width = Randbreite
border-style = Randstil
border-white = Weiß
border-black = Schwarz
border-mirrored-blur = Gespiegelter Blur

# PPI
ppi-select-image = Bild auswählen, um die PPI zu sehen.
ppi-inspecting = Bild wird untersucht…
ppi-dimensions-unavailable = Abmessungen nicht verfügbar.
ppi-quality-low = niedrig — wahrscheinlich weich
ppi-quality-ok = akzeptabel für den Druck
ppi-quality-sharp = scharf
ppi-aspect-warning = Ziel-Seitenverhältnis weicht von der Vorlage ab — das Bild wird beschnitten oder mit Rand versehen.

# Output card
card-output = AUSGABE
save-to = Speichern unter
change = Ändern…
choose = Wählen…
no-folder = Kein Ordner gewählt
format = Format
quality = Qualität
bit-depth = Bittiefe
effort = Aufwand
compression = Kompression
deflate-fast = Schnell
deflate-balanced = Ausgewogen
deflate-best = Bestes
bit-depth-8 = 8-Bit
bit-depth-16 = 16-Bit
source-other-depth = andere Bittiefe
note-16-tiff-available = 16-Bit-TIFF ist für die ausgewählte 16-Bit-Vorlage verfügbar.
note-16-tiff-8-source = 16-Bit-TIFF ist deaktiviert: Die Vorlage ist nur 8-Bit, wir erfinden keine Details.
note-16-tiff-needs-source = 16-Bit-TIFF benötigt eine Vorlage, die als echtes 16-Bit dekodiert.
note-16-tiff-only = 16-Bit-Ausgabe ist nur für TIFF verfügbar.

# Export status card
card-export-status = EXPORTSTATUS
no-export-running = Bisher kein Export aktiv.
export-started = { $count ->
    [one] Export gestartet — { $count } Worker
   *[other] Export gestartet — { $count } Worker
}

# Preview summary card
card-preview = VORSCHAU
softproof-profile = Softproof-Profil
clear = Entfernen
profile-fallback = Profil
no-softproof = Keine — kein Softproof
preview-window-hint = Die Vorschau öffnet sich in einem eigenen Fenster — frei skalierbar oder als Vollbild.
profile-change-hint = Eine Profiländerung erfordert eine Aktualisierung der Vorschau.
rendering-background = Vorschau wird im Hintergrund gerendert…
preview-size = Vorschau-Größe: { $width } × { $height } px
refresh-preview = Vorschau aktualisieren
show-window = Fenster anzeigen
cleared-softproof = Softproof-Profil entfernt.

# Progress / status messages
idle = Leerlauf
starting-preview = Vorschau wird gestartet
preview-ready = Vorschau bereit
select-image-to-preview = Zuerst ein Bild zur Vorschau auswählen
pick-output-folder = Zuerst einen Ausgabeordner wählen
add-images-to-export = Zuerst Bilder zum Exportieren hinzufügen
loading-image = Bild wird geladen
applying-border = Rand wird angewendet
simulating-compression = Kompression wird simuliert
applying-icc = ICC-Softproof wird angewendet
preparing-texture = Textur wird vorbereitet

# File entry
entry-inspecting = Wird untersucht…
entry-error = Fehler: { $error }
entry-dimensions-depth = { $width } × { $height}px · { $depth }
entry-dimensions = { $width } × { $height }px
entry-ready = Bereit

# Preview window
preview-title = Preprint — Vorschau
softproof-on = Softproof an
softproof-off = Softproof aus
no-softproof-profile = Kein Softproof-Profil
magnifier = Lupe
zoom = Zoom
lens = Linse
magnifier-hint = Linke Maustaste auf dem Bild halten, um zu inspizieren.
close = Schließen
fullscreen = Vollbild
exit-fullscreen = Vollbild beenden
no-preview-yet = Noch keine Vorschau gerendert — auf {"\""}Vorschau aktualisieren{"\""} klicken.

# Compression preview labels
compression-jpeg = Kompressionsvorschau: JPEG q{ $quality }
compression-png = Kompressionsvorschau: PNG Aufwand { $level }
compression-tiff-lzw = Kompressionsvorschau: TIFF LZW
compression-tiff-zip = Kompressionsvorschau: TIFF ZIP ({ $level })
