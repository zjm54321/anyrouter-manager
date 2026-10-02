# Licensing and third-party notices

## Original project code

Copyright (c) 2026 AnyRouter Manager contributors.

The original code and project-authored adaptations in AnyRouter Manager are
licensed under the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later version
(SPDX: `GPL-3.0-or-later`). See [LICENSE](LICENSE) for the full GPLv3 text.
This software is distributed without any warranty, including the implied
warranties of merchantability or fitness for a particular purpose.

This grant does not relicense third-party code, dependencies, or browser binaries.
Their own licenses and notices remain applicable. These notices identify adapted
source and the browser's separate licensing; they are not an exhaustive inventory
of transitive dependencies. Preserve applicable dependency notices when
distributing those dependencies.

## anyrouter-check-in — BSD-2-Clause

- Upstream: <https://github.com/millylee/anyrouter-check-in>
- Reference commit: `3b16662142c68703ea3e79c77c06a8cf6bba7990`
- Original license: <https://raw.githubusercontent.com/millylee/anyrouter-check-in/3b16662142c68703ea3e79c77c06a8cf6bba7990/LICENSE>
- Scope: the login flow in `tools/browser-helper/` was inspired by and adapted
  from this upstream project, with project-specific changes. This is not a claim
  that the entire upstream project was copied.

The original BSD notice, conditions, and disclaimer are retained below:

```text
BSD 2-Clause License

Copyright (c) 2025, Milly

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## CloakBrowser source and wrapper — MIT

- Upstream: <https://github.com/CloakHQ/CloakBrowser>
- Reference commit: `7e626ee7a1b0e72ab2c9b98315c36302148df1ce`
- Original license: <https://raw.githubusercontent.com/CloakHQ/CloakBrowser/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/LICENSE>
- Scope: `nix/cloak-browser.nix` adapts the upstream official Nix flake's
  packaging for this project, with local packaging modifications. The Python
  `cloakbrowser==0.5.11` wrapper dependency is also MIT-licensed; it is not
  relicensed wholesale under this project's GPL grant.

The original MIT notice and license are retained below:

```text
MIT License

Copyright (c) 2026 CloakHQ

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Chromium and bundled components — BSD-style and other upstream licenses

CloakBrowser includes Chromium, whose source uses a BSD-style license, together
with components under their own licenses. See the
[Chromium source license](https://chromium.googlesource.com/chromium/src/+/refs/heads/main/LICENSE)
and the component-specific copyright notices, license conditions, and disclaimers
accompanying the official browser archive. Chromium's license does not replace
the separate CloakBrowser binary terms below.

The container extracts the complete official archive and copies its contents,
including its original notices, into `/opt/cloakbrowser/` without pruning them.
The project adds the pinned upstream license files as
`/opt/cloakbrowser/PROJECT-WRAPPER-LICENSE` and
`/opt/cloakbrowser/PROJECT-BINARY-LICENSE.md`; these do not overwrite or replace
the archive's own notices. Preserve all applicable original archive notices when
redistributing the browser or image.

## CloakBrowser Chromium binary — separate proprietary terms

Copyright (c) 2026 CloakHQ. All rights reserved.

The compiled browser binary is **not** covered by the wrapper's MIT license or
this project's GPL grant. Its separate terms are the
[CloakBrowser Binary License](https://raw.githubusercontent.com/CloakHQ/CloakBrowser/7e626ee7a1b0e72ab2c9b98315c36302148df1ce/BINARY-LICENSE.md),
version 1.0 (February 2026). Refer to that full license and retain the license
files and notices accompanying the official download. The container bundles the
official, hash-verified, unmodified Linux x64 archive for browser version
`146.0.7680.177.5` and includes the full pinned binary license at
`/opt/cloakbrowser/PROJECT-BINARY-LICENSE.md`.

The license permits personal and commercial use. Its container provision states:

> **Internal use** — You may store and run the unmodified Binary within internal infrastructure, including Docker images, VM templates, CI runners, container registries, and artifact repositories (e.g., Artifactory, Nexus), solely for your organization's internal operational purposes.

Thus private personal/internal container use is not categorically prohibited.
Listing CloakBrowser as a dependency, with users downloading the binary directly
from official CloakHQ channels, is also expressly permitted and is not
redistribution. Internal mirrors are subject to the stated internal-use terms.

The binary license restricts redistribution and modification. Bundling,
embedding, or pre-installing the binary in products, hosted services, or cloud
artifacts distributed to third parties requires a separate OEM license; this
also includes using it to serve third-party customers. Do not publicly distribute
the binary as though this project's GPL license grants that right.

Licensing of the Nix packaging source does not grant additional rights in the
browser binary, nor does this notice declare all binary alterations permitted.
Actual browser use remains subject to the separate binary terms and any
applicable upstream component licenses. Obtain the binary through official
CloakHQ channels and consult CloakHQ for OEM/SaaS permissions when required.

### Owner-confirmed permission for this public-container delivery

For this delivery only, the deployment owner has confirmed explicit permission
to redistribute the pinned `146.0.7680.177.5` browser in a public container.
The public publisher relies on that owner confirmation and remains responsible
for staying within the obtained grant. No private grant text is recorded here.

This confirmation does **not** change the upstream binary license, assert that
its default terms permit public redistribution, relicense the browser under GPL
or MIT, or grant redistribution rights to other publishers. Other distributions,
browser versions, or uses must be covered by their own applicable permissions.
