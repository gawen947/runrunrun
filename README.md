# runrunrun

**A file and URL opener that runs the right thing.**

## Why runrunrun?

Traditional desktop file openers like `xdg-open` often make assumptions about your preferences, choosing browsers and applications based on your desktop environment rather than your actual needs. With `.desktop` files scattered across numerous locations, it's nearly impossible to get a clear view of what opens what. You'd need to manually inspect each file to understand the associations, and every desktop environment tries to override your preferences with its own.

`runrunrun` (or `rrr` for short) takes a different approach: simple file and URL handling through explicit configuration.

## Philosophy

The core philosophy of `runrunrun` is that opening files and URLs should be straightforward and transparent. No guessing what your preferred browser might be based on your desktop environment. No complex cascade of MIME types, desktop files, and environment-specific overrides. Just simple patterns that match files and URLs to the programs you want.

The same user might need different applications in different situations. Profiles address this need. Overrides are easy and predictable through simple rule ordering. File extensions and URL schemes provide the information needed to select the right application. Unlike desktop-centric tools, `rrr` also works in terminal environments. This makes it suitable for servers and minimal setups where simplicity matters more than desktop integration.

## Core Concepts

### Basic Pattern Matching

Match files by extension to applications:
```
*.pdf    qpdf
*.jpg    feh
*.ogg    audacious
```

Match URL schemes:
```
https://*    firefox
mailto:*     thunderbird
magnet:*     transmission-gtk
```

### Pattern Precedence

Later patterns override earlier ones:
```
*.txt    mousepad
*.txt    leafpad    # This wins
```

### Regular Expressions

Use `~` prefix for regex patterns (higher priority than globs):
```
~\.jpe?g$           gimp
~^IMG_[0-9]+\.png$  darktable
```

### Aliases

Define reusable actions:
```
[browser]    firefox
https://*    [browser]
http://*     [browser]
*.html       [browser]
```

### Profiles

Switch between different configurations for different contexts:
```
:profile minimal
*.txt    cat
*.log    less

:profile desktop
*.txt    gedit
*.log    gnome-system-log
```

### Includes

Organize your configuration across multiple files with `:include`. This accepts individual files or entire directories (loaded recursively):
```
:include ~/.config/rrr/web.conf
:include ~/.config/rrr/development.conf
:include /etc/rrr.d/
```

### Import (Future Version)

The `:import` directive will allow importing desktop files. It will extract MIME types from `.desktop` files and generate appropriate rules automatically. You'll be able to import files individually or scan directories recursively.

## Usage

```bash
# Open a file
rrr document.pdf

# Open a URL
rrr https://example.com

# Query what would run
rrr -q image.jpg

# Use a different profile
rrr -p work https://intranet.local
# Or with environment variable
RRR_PROFILE=work rrr https://intranet.local

# Dry run to test configuration
rrr -n *.txt
```

## Configuration

Default configuration locations:
- `/usr/local/etc/rrr.conf` or `/etc/rrr.conf` (depending on the OS)
- `$HOME/.config/rrr.conf`

For a more complete example configuration, see `docs/sample.conf` in the repository.