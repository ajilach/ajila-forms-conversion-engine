import csv
import sys

old = {}
with open('results-baseline.csv') as f:
    for row in csv.DictReader(f):
        old[row['form_code']] = row

new = {}
with open('results.csv') as f:
    for row in csv.DictReader(f):
        new[row['form_code']] = row

header = "{:<12} {:>8} {:>8} {:>8}  {:>8} {:>8} {:>8}  {:>8} {:>8} {:>8}".format(
    'Form', 'Old TR', 'New TR', 'D TR', 'Old MTS', 'New MTS', 'D MTS', 'Old Tot', 'New Tot', 'D Tot')
print(header)
print('-' * 105)

changes = []
for code in sorted(old.keys()):
    if code == 'AVERAGE':
        continue
    if code not in new:
        continue
    o = old[code]
    n = new[code]
    otr = float(o['translation_rating'])
    ntr = float(n['translation_rating'])
    omts = float(o['missing_translation_score'])
    nmts = float(n['missing_translation_score'])
    ots = float(o['total_score'])
    nts = float(n['total_score'])
    dtr = ntr - otr
    dmts = nmts - omts
    dts = nts - ots
    if abs(dtr) > 0.0005 or abs(dmts) > 0.0005 or abs(dts) > 0.0005:
        changes.append((code, otr, ntr, dtr, omts, nmts, dmts, ots, nts, dts))

for row in sorted(changes, key=lambda x: x[-1]):
    code, otr, ntr, dtr, omts, nmts, dmts, ots, nts, dts = row
    print("{:<12} {:>8.3f} {:>8.3f} {:>+8.3f}  {:>8.3f} {:>8.3f} {:>+8.3f}  {:>8.3f} {:>8.3f} {:>+8.3f}".format(
        code, otr, ntr, dtr, omts, nmts, dmts, ots, nts, dts))

# Print average
o_avg = old.get('AVERAGE')
n_avg = new.get('AVERAGE')
if o_avg and n_avg:
    print('-' * 105)
    otr = float(o_avg['translation_rating'])
    ntr = float(n_avg['translation_rating'])
    omts = float(o_avg['missing_translation_score'])
    nmts = float(n_avg['missing_translation_score'])
    ots = float(o_avg['total_score'])
    nts = float(n_avg['total_score'])
    print("{:<12} {:>8.3f} {:>8.3f} {:>+8.3f}  {:>8.3f} {:>8.3f} {:>+8.3f}  {:>8.3f} {:>8.3f} {:>+8.3f}".format(
        'AVERAGE', otr, ntr, ntr-otr, omts, nmts, nmts-omts, ots, nts, nts-ots))

print("\nTotal changed forms:", len(changes))
improved = sum(1 for c in changes if c[-1] > 0.0005)
regressed = sum(1 for c in changes if c[-1] < -0.0005)
print("Improved:", improved)
print("Regressed:", regressed)
