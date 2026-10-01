local th = require("maki.test_helpers")
local helpers = require("tests.helpers")
local case = th.case
local idx = helpers.idx
local has = helpers.has
local lacks = helpers.lacks

case("clojure_core_forms", function()
  local src = [=[
(ns my.app
  (:require [clojure.string :as str]
            [clojure.set :refer [union]]
            [schema.core :as s])
  (:import (java.util Date Calendar)))

(def ^:private answer 42)

(defonce cache (atom {}))

(defn greet
  "doc"
  [name]
  (str "hi " name))

(defn multi
  ([x] x)
  ([x y] (+ x y)))

(defn- helper [x] (inc x))

(defmacro with-thing [& body] `(do ~@body))

(defmulti area :type)

(defmethod area :rect [s] (* (:w s) (:h s)))

(defprotocol P
  (size [this])
  (grow [this n] "doc"))

(defrecord Point [x y]
  P
  (size [this] 2))

(deftype Box [v]
  P
  (size [this] 1))

(declare pending-a pending-b)

(defmethod area :circle [c] 3)

(defn view [s] [:div s])
]=]
  local out = idx(src, "clojure")
  has(out, {
    "mod: [",
    "my.app",
    "require: clojure.string :as str",
    "require: clojure.set :refer [union]",
    "java.util.{Calendar, Date}",
    "consts:",
    "^:private answer = 42",
    "cache = (atom {})",
    "fns:",
    "greet [name]",
    "multi [x] [x y]",
    "helper [x]",
    "macros:",
    "with-thing [& body]",
    "impls:",
    "area :rect [s]",
    "area :circle [c]",
    "types:",
    "size [this]",
    "grow [this n]",
    "Point [x y]",
    "Box [v]",
    "pending-a",
    "pending-b",
    "view [s]",
  })
  lacks(out, {
    '(str "hi " name)',
    "(+ x y)",
    "[:div",
  })
end)

case("clojure_spec_and_schema", function()
  local src = [=[
(ns my.spec
  (:require [clojure.spec.alpha :as spec]
            [schema.core :as s]))

(spec/def ::email (spec/and string? #(re-find #"@" %)))

(s/def name :- s/Str)

(s/defn age :- s/Int [p] (:age p))

(s/defn described :- s/Int "doc" [p] (:age p))

(s/defrecord User [id :- s/Int name :- s/Str])
]=]
  local out = idx(src, "clojure")
  has(out, {
    "rules:",
    "::email",
    "name :- s/Str",
    "age [p]",
    "described [p]",
    "User [id :- s/Int name :- s/Str]",
  })
  lacks(out, { "email =" })
end)

case("clojure_unknown_def_forms", function()
  local src = [=[
(ns my.app
  (:require [clojure.test :refer [deftest is]]))

(deftest my-test
  (is (= 1 1)))

(defthing thing-one :kind-a)

(my.ns/defwhatever x y z)

(defwidget Card
  [props]
  (:div props))

(defwidget Tabs
  ([items]
   (:ul items))
  ([items selected]
   (:ul items selected)))

(defcomponent c [p] [:div p])
]=]
  local out = idx(src, "clojure")
  has(out, {
    "forms:",
    "deftest my-test [4-5]",
    "defthing thing-one [7]",
    "my.ns/defwhatever x [9]",
    "defwidget Card [props] [11-13]",
    "defwidget Tabs [items] [items selected] [15-19]",
    "defcomponent c [p] [21]",
  })
  lacks(out, { "[:div" })
end)

case("clojure_discovers_nested_definitions", function()
  local src = [=[
(let [x 10]
  (defn a [] x)
  (defn b [y] (+ x y)))

(when true
  (def value 1)
  (let [inner 2]
    (defn nested [z] (+ inner z))))
]=]
  local out = idx(src, "clojure")
  has(out, {
    "a [] [2]",
    "b [y] [3]",
    "value = 1 [6]",
    "nested [z] [8]",
  })
end)

case("clojure_ignores_commented_and_discarded", function()
  local src = [=[
(def real-one 1)

(comment
  (def commented-out 2))

(cljs.core/comment
  (def cljs-commented-out 3))

#_(def discarded 4)

(def quoted 'sym)

(def data {:k (def not-a-def 5)})

'(def quoted-out 6)

`(def syntax-quoted 7)

#tag (def tagged 8)
]=]
  local out = idx(src, "clojure")
  has(out, {
    "real-one = 1",
    "commented-out = 2",
    "cljs-commented-out = 3",
    "quoted = 'sym",
    "not-a-def = 5",
  })
  lacks(out, {
    "discarded",
    "quoted-out",
    "syntax-quoted",
    "tagged",
  })
end)

case("clojure_multiline_values_and_metadata", function()
  local src = [=[
(def short-value
  {:routes [["/" {:get home}]
            ["/about" {:get about}]]})

(def ^{:doc "first line of doc
               second line of doc"
       :added "1.0"}
  thing
  1)

(defonce state
  (atom {:count 0}))

(def ^{:doc "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
  long-meta
  1)
]=]
  local out = idx(src, "clojure")
  has(out, {
    'short-value = {:routes [["/" {:get home}] ["/about" {:get about}]]}',
    "thing = 1",
    "state = (atom {:count 0})",
    '^{:doc "aaaaaaaaaaaaaaaa',
    "[truncated]",
  })
  lacks(out, { "second line of doc\n" })
end)

case("clojure_runtime_loads", function()
  local src = [=[
(ns my.loader
  (:require [app.core :as app]
            [app.optional :as-alias optional]
            (shared [util :as u] (nested [data :refer [def]]))
            [schema.core :refer [def] :rename {def schema-def}])

  (:require-macros [macro.lib :refer-macros [defmacro]])
  (:use [use.lib :only [def] :rename {def use-def}])
  (:use-macros [use.macros :only [defmacro]])
  (:load "bootstrap" "shared/helpers"))

(require 'extra.core
         '[extra.util :as util]
         '(prefixed one [two :as two])
         '(deep (more [thing :as deep-thing]))
         :reload
         (symbol "dynamic.core"))

(schema-def named :- s/Int 1)
(use-def named-use 1)
(defmacro named-macro [] nil)


(load "runtime/helpers" '"quoted/helpers" dynamic-path)

(load-file "script.clj" (io/file "computed.clj"))
]=]
  local out = idx(src, "clojure")
  has(out, {
    "require: app.core :as app",
    "alias: app.optional :as-alias optional",
    "require: shared.util :as u",
    "require: shared.nested.data :refer [def]",
    "require: schema.core :refer [def] :rename {def schema-def}",
    "require-macros: macro.lib :refer-macros [defmacro]",
    "use: use.lib :only [def] :rename {def use-def}",
    "use-macros: use.macros :only [defmacro]",
    'load: "bootstrap"',
    'load: "shared/helpers"',
    "require: extra.core",
    "require: extra.util :as util",
    "require: prefixed.one",
    "require: prefixed.two :as two",
    "require: deep.more.thing :as deep-thing",
    'load: "runtime/helpers"',
    'load: "quoted/helpers"',
    'load-file: "script.clj"',
  })
  lacks(out, { "dynamic.core", "dynamic-path", "computed.clj" })
end)

case("clojure_reader_conditionals", function()
  local src = [=[
(ns my.dual)

#?(:clj
   (defn platform [] "jvm")
   :cljs
   (defn platform [] "js"))

#?@(:clj [(def extra-clj 1)] :cljs [(def extra-cljs 2)])

(def shared 3)
]=]
  local out = idx(src, "clojure")
  has(out, {
    "platform []",
    "extra-clj = 1",
    "extra-cljs = 2",
    "shared = 3",
  })
end)

case("clojure_unknown_def_forms_skip_calls", function()
  local src = [=[
(defn handler [req]
  (d/deferred)
  (default req)
  (defaults req {:a 1})
  (deferred-thing req))

(defmacro defx [sym val] `(def ~sym ~val))

(defx thing 42)

(defentity user [id])
]=]
  local out = idx(src, "clojure")
  has(out, {
    "handler [req]",
    "defx [sym val]",
    "defaults req",
    "defx thing",
    "defentity user [id]",
  })
  lacks(out, { "d/deferred", "default req", "deferred-thing" })
end)

case("clojure_def_without_value_and_broken_name", function()
  local src = [=[
(def x)

(defn (broken-name) [a] a)
]=]
  local out = idx(src, "clojure")
  has(out, { "consts:", "x [1]" })
  lacks(out, { "forms:", "broken-name" })
end)

case("clojure_cljs_string_and_unknown_option_libspecs", function()
  local src = [=[
(ns my.ui
  (:require ["react" :as react]
            ["dayjs" :default dayjs]
            [reagent.core :as r :include-macros true]
            [clojure.spec.alpha :as spec :custom {:x 1}]))

(spec/def ::email string?)
]=]
  local out = idx(src, "clojure")
  has(out, {
    'require: "react" :as react',
    'require: "dayjs" :default dayjs',
    "require: reagent.core :as r :include-macros true",
    "require: clojure.spec.alpha :as spec :custom {:x 1}",
    "rules:",
    "::email",
  })
end)

case("clojure_def_docstring_skipped", function()
  local src = [=[
(def default-timeout "How long to wait before giving up." 5000)

(def greeting "hello")
]=]
  local out = idx(src, "clojure")
  has(out, { "default-timeout = 5000", 'greeting = "hello"' })
  lacks(out, { "How long to wait" })
end)

case("clojure_import_vector_form", function()
  local src = [=[
(ns my.app
  (:import [java.util Date]))
]=]
  local out = idx(src, "clojure")
  has(out, { "java.util.Date" })
end)

case("clojure_reader_conditionals_in_ns", function()
  local src = [=[
(ns my.dual
  (:require #?(:clj [x.y :as z] :cljs [clojure.spec.alpha :as sp])
            #?@(:cljs [[a.b :as c] [schema.core :as s]]))
  #?(:clj (:import [java.io File])))

(sp/def ::id int?)

(s/def limit :- s/Int 10)
]=]
  local out = idx(src, "clojure")
  has(out, {
    "require: x.y :as z",
    "require: clojure.spec.alpha :as sp",
    "require: a.b :as c",
    "require: schema.core :as s",
    "java.io.File",
    "rules:",
    "::id",
    "limit :- s/Int",
  })
end)
