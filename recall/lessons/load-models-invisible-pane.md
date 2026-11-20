# load-models-invisible-pane
- trigger: clicking Load models in the Providers pane did nothing visible — the tick panel only exists in the Models pane, which was hidden, so the user could not select any model
- rule: when a button's result panel lives in a different pane, switch to that pane first (show('models')) before unhiding the panel
- applies: admin.html loadProvModels, any cross-pane action
