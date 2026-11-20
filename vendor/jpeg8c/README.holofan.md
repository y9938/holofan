# IJG 8c integration notes

This directory contains unmodified JPEG decoder sources from Independent JPEG Group release 8c (16-Jan-2011), obtained from `https://www.ijg.org/files/jpegsrc.v8c.tar.gz`. The upstream `README` is retained unchanged as required by its redistribution terms.

The application compiles the decoder into its executable and calls it through `src/jpeg8c_wrapper.c`. `jconfig.h` is the upstream configure-generated Linux configuration; its `INLINE` definition is selected per compiler for Windows compatibility. No IJG source file is patched. The C ABI wrapper and build integration are HoloFan code.

When distributing only an executable, the IJG notice requires accompanying documentation to state: “this software is based in part on the work of the Independent JPEG Group”. Source distributions must include the upstream README and notice.
